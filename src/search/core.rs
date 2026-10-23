//! Exact reverse-search states and single-action transitions.

use std::io;

use crate::io::{TriCategory, TriOutcome};
use crate::othello::{flip, get_moves, Board, Position, CENTER_MASK};
use crate::record::{GoalMatch, PlyAction, PositionTransform, ReachabilityRecord};
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

/// Every successful search carries a replayed certificate for its exact goal.
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
    pub reverse_actions: Vec<PlyAction>,
}

impl SearchNode {
    pub fn root(position: Position) -> Self {
        Self {
            position,
            reverse_actions: Vec::new(),
        }
    }

    pub fn preceding(&self, position: Position, action: PlyAction) -> Self {
        let mut reverse_actions = self.reverse_actions.clone();
        reverse_actions.push(action);
        Self {
            position,
            reverse_actions,
        }
    }
}

pub(crate) fn found(
    table: &ForwardTable,
    goal: Position,
    node: &SearchNode,
    nodes: usize,
) -> io::Result<SearchOutcome> {
    let mut actions = table.actions_to(node.position).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "meeting position has no forward path",
        )
    })?;
    actions.extend(node.reverse_actions.iter().rev().copied());
    let record = ReachabilityRecord {
        input_index: 1,
        start: table.start(),
        goal,
        goal_match: GoalMatch::Exact,
        transform: PositionTransform::IDENTITY,
        actions,
    };
    record
        .verify()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(SearchOutcome::Found { record, nodes })
}

/// Each successor is exactly one placement or one legal pass. A terminal
/// position has no successors, and its final absolute turn is preserved.
pub fn successors(position: Position) -> Vec<(Position, PlyAction)> {
    let Board { player, opponent } = position.board;
    let side_to_move = position.side_to_move.opposite();
    let mut moves = get_moves(player, opponent);
    if moves == 0 {
        return if get_moves(opponent, player) != 0 {
            vec![(
                Position {
                    board: position.board.swapped(),
                    side_to_move,
                },
                PlyAction::Pass,
            )]
        } else {
            Vec::new()
        };
    }
    let mut next = Vec::new();
    while moves != 0 {
        let square = moves.trailing_zeros() as usize;
        moves &= moves - 1;
        let flips = flip(square, player, opponent);
        next.push((
            Position {
                board: Board::new(opponent & !flips, player | flips | (1 << square)),
                side_to_move,
            },
            PlyAction::Play(square),
        ));
    }
    next
}

/// Enumerate every one-action predecessor, without geometric or color
/// canonicalization. A reverse pass requires that the current player can move
/// and its opponent cannot; it cannot create a pass after termination.
pub fn predecessors(position: Position) -> Vec<(Position, PlyAction)> {
    let mut result = Vec::new();
    if let Some(previous) = pass_predecessor(position) {
        result.push((previous, PlyAction::Pass));
    }
    append_placement_predecessors(position, u64::MAX, &mut result);
    result
}

pub(crate) fn pass_predecessor(position: Position) -> Option<Position> {
    let Board { player, opponent } = position.board;
    if get_moves(opponent, player) == 0 && get_moves(player, opponent) != 0 {
        Some(Position {
            board: position.board.swapped(),
            side_to_move: position.side_to_move.opposite(),
        })
    } else {
        None
    }
}

pub(crate) fn append_placement_predecessors(
    position: Position,
    candidates: u64,
    result: &mut Vec<(Position, PlyAction)>,
) {
    let Board { player, opponent } = position.board;
    let side_to_move = position.side_to_move.opposite();
    let mut squares = opponent & !CENTER_MASK & candidates;
    while squares != 0 {
        let square = squares.trailing_zeros() as usize;
        squares &= squares - 1;
        // A last move cannot have left any capture in its original colors.
        if flip(square, opponent ^ (1 << square), player) != 0 {
            continue;
        }
        // Every direction can undo a prefix of the current opponent's ray,
        // leaving at least one bounding stone. The Cartesian product includes
        // independently flipped directions; at least one must be nonempty.
        let mut sets = vec![0_u64];
        // Keep the placement DFS branch order: -8, +8, -1, +1, -9, +9, -7, +7.
        for (dx, dy) in [
            (0, -1),
            (0, 1),
            (-1, 0),
            (1, 0),
            (-1, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
        ] {
            let mut ray = Vec::new();
            let (mut x, mut y) = (square as i32 % 8 + dx, square as i32 / 8 + dy);
            while (0..8).contains(&x) && (0..8).contains(&y) {
                let bit = 1_u64 << (8 * y + x);
                if opponent & bit == 0 {
                    break;
                }
                ray.push(bit);
                x += dx;
                y += dy;
            }
            let previous_len = sets.len();
            let mut prefix = 0;
            for &bit in ray.iter().take(ray.len().saturating_sub(1)) {
                prefix |= bit;
                for index in 0..previous_len {
                    sets.push(sets[index] | prefix);
                }
            }
        }
        for flips in sets.into_iter().skip(1) {
            let previous = Board::new(opponent & !(flips | (1 << square)), player | flips);
            // An unflipped direction may otherwise imply another capture.
            if flip(square, previous.player, previous.opponent) == flips {
                result.push((
                    Position {
                        board: previous,
                        side_to_move,
                    },
                    PlyAction::Play(square),
                ));
            }
        }
    }
}

pub(crate) fn viable(position: Position, use_lp: bool, occupancy_cache: bool) -> io::Result<bool> {
    use crate::filters::{
        impossible_edges::check_edge_patterns, linear_programming::check_lp,
        occupancy::check_occupancy, seg3::check_seg3_more,
    };
    let Board { player, opponent } = position.board;
    if !check_occupancy(player | opponent)
        || !check_edge_patterns(player, opponent)
        || !check_seg3_more(player, opponent, occupancy_cache)
    {
        return Ok(false);
    }
    if use_lp {
        let result = check_lp(player, opponent, false, occupancy_cache)?;
        if let crate::filters::FilterResult::Undetermined { reason } = &result {
            eprintln!(
                "info: LP undetermined for {}: {}; retaining this search node",
                crate::io::format_position(position),
                reason
            );
        }
        return Ok(result.passes());
    }
    Ok(true)
}
