//! Endpoint selection and classification shared by exact reachability solvers.

use crate::io::{TriCategory, TriOutcome};
use crate::othello::{validate_board, BoardSymmetry, Position};
use crate::record::{GoalMatch, PlyAction, PositionTransform, ReachabilityRecord};
use std::collections::HashSet;

/// One classification per input occurrence, with a 1-based index.
/// Reachable always carries a replay-verified record of the requested query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GoalClassification {
    Reachable {
        record: ReachabilityRecord,
    },
    Unreachable {
        input_index: usize,
        goal: Position,
        goal_match: GoalMatch,
    },
    Unknown {
        input_index: usize,
        goal: Position,
        goal_match: GoalMatch,
    },
}

impl GoalClassification {
    pub fn goal(&self) -> Position {
        match self {
            Self::Reachable { record } => record.goal,
            Self::Unreachable { goal, .. } | Self::Unknown { goal, .. } => *goal,
        }
    }

    pub fn input_index(&self) -> usize {
        match self {
            Self::Reachable { record } => record.input_index,
            Self::Unreachable { input_index, .. } | Self::Unknown { input_index, .. } => {
                *input_index
            }
        }
    }

    pub fn goal_match(&self) -> GoalMatch {
        match self {
            Self::Reachable { record } => record.goal_match,
            Self::Unreachable { goal_match, .. } | Self::Unknown { goal_match, .. } => *goal_match,
        }
    }

    pub fn record(&self) -> Option<&ReachabilityRecord> {
        match self {
            Self::Reachable { record } => Some(record),
            _ => None,
        }
    }
}

impl TriOutcome for GoalClassification {
    fn category(&self) -> TriCategory {
        match self {
            Self::Reachable { .. } => TriCategory::Ok,
            Self::Unreachable { .. } => TriCategory::Ng,
            Self::Unknown { .. } => TriCategory::Unknown,
        }
    }
}

/// Result for one exact endpoint. A reachable result supplies every move and pass.
#[derive(Debug)]
pub enum ExactOutcome {
    Reachable(Vec<PlyAction>),
    Unreachable,
    Unknown,
}

/// Enumerate endpoints modulo geometric symmetries that fix the start.
/// Absolute turns remain distinct even when their relative boards coincide.
///
/// Candidates whose absolute turn admits a history without passes come
/// first: every placement changes the turn, so the other candidates need an
/// odd number of passes, which is rare. A query stops at its first reachable
/// candidate, so this order tends to settle reachable queries sooner.
pub fn candidates(
    start: Position,
    goal: Position,
    goal_match: GoalMatch,
) -> Vec<(PositionTransform, Position)> {
    if goal_match == GoalMatch::Exact {
        return vec![(PositionTransform::IDENTITY, goal)];
    }
    let stabilizer: Vec<_> = BoardSymmetry::ALL
        .into_iter()
        .filter(|symmetry| symmetry.apply_position(start) == start)
        .collect();
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for transform in goal_match.transforms() {
        let candidate = transform.apply_position(goal);
        let canonical = stabilizer
            .iter()
            .map(|symmetry| symmetry.apply_position(candidate))
            .min()
            .expect("identity fixes every start");
        if seen.insert(canonical) {
            candidates.push((transform, candidate));
        }
    }
    let needs_pass = |candidate: &Position| {
        let placements = candidate
            .board
            .popcount()
            .saturating_sub(start.board.popcount());
        placements.is_multiple_of(2) != (candidate.side_to_move == start.side_to_move)
    };
    candidates.sort_by_key(|(_, candidate)| needs_pass(candidate));
    candidates
}

/// One reachable endpoint proves the query; unreachability requires all endpoints.
/// Execution errors propagate, and `None` is reserved for generation-only runs.
pub fn solve_query(
    start: Position,
    goal: Position,
    input_index: usize,
    goal_match: GoalMatch,
    mut solve: impl FnMut(Position, PositionTransform) -> Result<Option<ExactOutcome>, String>,
) -> Result<Option<GoalClassification>, String> {
    if input_index == 0 {
        return Err("input index must be at least 1".to_owned());
    }
    validate_board(&start.board).map_err(|error| format!("invalid start: {error:?}"))?;
    validate_board(&goal.board).map_err(|error| format!("invalid goal: {error:?}"))?;
    let mut unknown = false;
    let mut classified = false;
    let mut generated_only = false;
    for (transform, candidate) in candidates(start, goal, goal_match) {
        let Some(result) = solve(candidate, transform)? else {
            if classified {
                return Err("query mixed generation-only and classified endpoints".to_owned());
            }
            generated_only = true;
            continue;
        };
        if generated_only {
            return Err("query mixed generation-only and classified endpoints".to_owned());
        }
        classified = true;
        match result {
            ExactOutcome::Reachable(actions) => {
                let record = ReachabilityRecord {
                    input_index,
                    start,
                    goal,
                    goal_match,
                    transform,
                    actions,
                };
                record.verify()?;
                return Ok(Some(GoalClassification::Reachable { record }));
            }
            ExactOutcome::Unreachable => {}
            ExactOutcome::Unknown => unknown = true,
        }
    }
    if !classified {
        Ok(None)
    } else if unknown {
        Ok(Some(GoalClassification::Unknown {
            input_index,
            goal,
            goal_match,
        }))
    } else {
        Ok(Some(GoalClassification::Unreachable {
            input_index,
            goal,
            goal_match,
        }))
    }
}
