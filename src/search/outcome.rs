//! Reverse-search outcomes and witness reconstruction.

use std::io;

use crate::io::{TriCategory, TriOutcome};
use crate::othello::{Board, BoardSymmetry, Position};
use crate::record::{replay, PlyAction, ReachabilityRecord};
use crate::search::forward::ForwardTable;

/// A resource stop is distinct from a proof that the search was exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResult {
    Found,
    NotFound,
    Unknown,
}

impl TriOutcome for SearchResult {
    fn category(&self) -> TriCategory {
        match self {
            Self::Found => TriCategory::Ok,
            Self::NotFound => TriCategory::Ng,
            Self::Unknown => TriCategory::Unknown,
        }
    }
}

/// Every successful search carries a replayed certificate whose endpoint
/// satisfies the requested `goal_match` condition.
#[derive(Debug, Clone)]
pub enum SearchOutcome {
    Found {
        record: ReachabilityRecord,
        nodes: usize,
    },
    NotFound {
        nodes: usize,
    },
    Unknown {
        nodes: usize,
    },
}

impl SearchOutcome {
    pub fn result(&self) -> SearchResult {
        match self {
            Self::Found { .. } => SearchResult::Found,
            Self::NotFound { .. } => SearchResult::NotFound,
            Self::Unknown { .. } => SearchResult::Unknown,
        }
    }

    pub fn nodes(&self) -> usize {
        match self {
            Self::Found { nodes, .. } | Self::NotFound { nodes } | Self::Unknown { nodes } => {
                *nodes
            }
        }
    }
}

/// A backward path stores actions from the goal towards this node. Reversing
/// them gives the forward suffix from `position` to the requested goal.
#[derive(Debug, Clone)]
pub(crate) struct SearchNode {
    pub position: Position,
    pub reverse_actions: Vec<u8>,
}

impl SearchNode {
    const PASS_ACTION: u8 = 64;

    pub fn root(position: Position) -> Self {
        Self {
            position,
            reverse_actions: Vec::new(),
        }
    }

    pub fn from_reverse_actions(
        position: Position,
        reverse_actions: impl IntoIterator<Item = PlyAction>,
    ) -> Self {
        Self {
            position,
            reverse_actions: reverse_actions
                .into_iter()
                .map(Self::encode_action)
                .collect(),
        }
    }

    pub fn preceding(&self, position: Position, action: PlyAction) -> Self {
        let mut reverse_actions = self.reverse_actions.clone();
        reverse_actions.push(Self::encode_action(action));
        Self {
            position,
            reverse_actions,
        }
    }

    pub fn push_reverse_action(&mut self, action: PlyAction) {
        self.reverse_actions.push(Self::encode_action(action));
    }

    fn encode_action(action: PlyAction) -> u8 {
        match action {
            PlyAction::Play(square) => {
                debug_assert!(square < 64);
                square as u8
            }
            PlyAction::Pass => Self::PASS_ACTION,
        }
    }

    fn decode_action(action: u8) -> PlyAction {
        if action == Self::PASS_ACTION {
            PlyAction::Pass
        } else {
            debug_assert!(action < Self::PASS_ACTION);
            PlyAction::Play(action as usize)
        }
    }
}

pub(crate) fn found(
    table: &ForwardTable,
    goal: Position,
    node: &SearchNode,
    nodes: usize,
) -> io::Result<SearchOutcome> {
    let (mut actions, start_symmetry) = table.actions_to(node.position).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "meeting position has no forward path",
        )
    })?;
    actions.extend(
        node.reverse_actions
            .iter()
            .rev()
            .copied()
            .map(SearchNode::decode_action),
    );
    for action in &mut actions {
        if let PlyAction::Play(square) = action {
            *square = transform_square(*square, start_symmetry);
        }
    }
    let trace = replay(table.start(), &actions)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let endpoint = *trace.last().expect("replay includes its starting position");
    let transform = table
        .goal_match()
        .transforms()
        .find(|transform| transform.apply_position(goal) == endpoint)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "reconstructed endpoint does not match the requested goal",
            )
        })?;
    let record = ReachabilityRecord {
        input_index: 1,
        start: table.start(),
        goal,
        goal_match: table.goal_match(),
        transform,
        actions,
    };
    record
        .verify()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(SearchOutcome::Found { record, nodes })
}

fn transform_square(square: usize, symmetry: BoardSymmetry) -> usize {
    debug_assert!(square < 64);
    symmetry
        .apply(Board::new(1_u64 << square, 0))
        .player
        .trailing_zeros() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_actions_round_trip_and_distinguish_pass_from_the_last_square() {
        let actions = [PlyAction::Play(0), PlyAction::Play(63), PlyAction::Pass];
        let node = SearchNode::from_reverse_actions(Position::initial(), actions);

        assert_eq!(
            node.reverse_actions
                .iter()
                .copied()
                .map(SearchNode::decode_action)
                .collect::<Vec<_>>(),
            actions
        );
        assert_ne!(node.reverse_actions[1], node.reverse_actions[2]);
    }
}
