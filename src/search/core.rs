//! Exact reverse-search states and single-action transitions.

use std::io;

use crate::io::{TriCategory, TriOutcome};
use crate::othello::{flip, get_moves, Board, BoardSymmetry, Position, CENTER_MASK};
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
    result.extend(PlacementPredecessors::new(position, candidates));
}

/// Enumerate one placement predecessor at a time, in the original DFS order.
///
/// Each direction contributes a proper prefix of its same-color ray. Mixed
/// radix digits enumerate their Cartesian product with the first direction as
/// the least significant digit, exactly matching the former `sets` vector.
/// Storage is fixed regardless of the number of predecessor combinations.
pub(crate) struct PlacementPredecessors {
    position: Position,
    squares: u64,
    square: usize,
    prefixes: [[u64; 7]; 8],
    radices: [u8; 8],
    digits: [u8; 8],
    active: bool,
}

impl PlacementPredecessors {
    pub(crate) fn new(position: Position, candidates: u64) -> Self {
        Self {
            position,
            squares: position.board.opponent & !CENTER_MASK & candidates,
            square: 0,
            prefixes: [[0; 7]; 8],
            radices: [1; 8],
            digits: [0; 8],
            active: false,
        }
    }

    fn next_square(&mut self) -> bool {
        let Board { player, opponent } = self.position.board;
        while self.squares != 0 {
            self.square = self.squares.trailing_zeros() as usize;
            self.squares &= self.squares - 1;
            // A last move cannot have left an existing capture unflipped.
            if flip(self.square, opponent ^ (1_u64 << self.square), player) != 0 {
                continue;
            }
            self.digits = [0; 8];
            // Keep the original order: -8, +8, -1, +1, -9, +9, -7, +7.
            for (direction, (dx, dy)) in [
                (0, -1),
                (0, 1),
                (-1, 0),
                (1, 0),
                (-1, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
            ]
            .into_iter()
            .enumerate()
            {
                let (mut x, mut y) = (self.square as i32 % 8 + dx, self.square as i32 / 8 + dy);
                let mut length = 0;
                let mut prefix = 0;
                while (0..8).contains(&x) && (0..8).contains(&y) {
                    let bit = 1_u64 << (8 * y + x);
                    if opponent & bit == 0 {
                        break;
                    }
                    // Store before adding this stone, so every nonempty
                    // prefix leaves this stone as its bounding endpoint.
                    self.prefixes[direction][length] = prefix;
                    prefix |= bit;
                    length += 1;
                    x += dx;
                    y += dy;
                }
                // Empty and one-stone rays both allow only the empty prefix.
                self.prefixes[direction][0] = 0;
                self.radices[direction] = length.max(1) as u8;
            }
            self.active = true;
            return true;
        }
        false
    }
}

impl Iterator for PlacementPredecessors {
    type Item = (Position, PlyAction);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if !self.active && !self.next_square() {
                return None;
            }
            // Advance before yielding, thereby omitting the all-zero choice.
            let mut direction = 0;
            while direction < 8 {
                self.digits[direction] += 1;
                if self.digits[direction] < self.radices[direction] {
                    break;
                }
                self.digits[direction] = 0;
                direction += 1;
            }
            if direction == 8 {
                self.active = false;
                continue;
            }
            let mut flips = 0;
            for direction in 0..8 {
                flips |= self.prefixes[direction][self.digits[direction] as usize];
            }
            let Board { player, opponent } = self.position.board;
            let previous = Board::new(opponent & !(flips | (1_u64 << self.square)), player | flips);
            // Distinct rays are disjoint. Every selected nonempty prefix has
            // a retained bounding stone, and unchanged directions cannot have
            // extra captures because next_square rejected those positions.
            debug_assert_ne!(flips, 0);
            debug_assert_eq!(flip(self.square, previous.player, previous.opponent), flips);
            return Some((
                Position {
                    board: previous,
                    side_to_move: self.position.side_to_move.opposite(),
                },
                PlyAction::Play(self.square),
            ));
        }
    }
}

pub(crate) fn viable(position: Position, use_lp: bool, occupancy_cache: bool) -> io::Result<bool> {
    use crate::filters::{
        impossible_edges::check_virtual_edge_patterns,
        linear_programming::check_lp,
        occupancy::{check_occupancy, occupancy_order, occupancy_order_cached},
        seg3::check_seg3_more_with_order,
    };
    let Board { player, opponent } = position.board;
    let occupied = player | opponent;
    if !check_occupancy(occupied) || !check_virtual_edge_patterns(player, opponent) {
        return Ok(false);
    }
    let order = if occupancy_cache {
        occupancy_order_cached(occupied)
    } else {
        occupancy_order(occupied)
    };
    if !check_seg3_more_with_order(player, opponent, &order) {
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

#[cfg(test)]
mod search_node_tests {
    use super::*;

    #[test]
    fn reverse_actions_use_the_external_bfs_byte_codes() {
        let actions = [PlyAction::Play(0), PlyAction::Play(63), PlyAction::Pass];
        let node = SearchNode::from_reverse_actions(Position::initial(), actions);

        assert_eq!(node.reverse_actions, [0, 63, 64]);
        assert_eq!(
            node.reverse_actions
                .iter()
                .copied()
                .map(SearchNode::decode_action)
                .collect::<Vec<_>>(),
            actions
        );
    }
}

#[cfg(test)]
mod placement_iterator_tests {
    use std::collections::HashSet;

    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::othello::Color;

    // Keep the original eager implementation as an independent enumeration
    // oracle, including its final replay filter and original Cartesian order.
    fn reference_placements(
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

    fn assert_same_order(position: Position, candidates: u64) {
        let mut expected = Vec::new();
        reference_placements(position, candidates, &mut expected);
        let actual: Vec<_> = PlacementPredecessors::new(position, candidates).collect();
        assert_eq!(
            actual, expected,
            "position={position:?}, candidates={candidates:#018x}"
        );
    }

    #[test]
    fn lazy_placements_match_eager_order_on_random_and_dense_boards() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x1a2e_5eed);
        for _ in 0..1024 {
            let occupied = rng.random::<u64>() | CENTER_MASK;
            let player = rng.random::<u64>() & occupied;
            let position = Position {
                board: Board::new(player, occupied & !player),
                side_to_move: if rng.random::<bool>() {
                    Color::Black
                } else {
                    Color::White
                },
            };
            assert_same_order(position, u64::MAX);
            assert_same_order(position, rng.random::<u64>());
        }
        // Full single-color boards exercise long rays and Cartesian products
        // in several directions, including the maximum proper-prefix length.
        for board in [Board::new(0, u64::MAX), Board::new(u64::MAX, 0)] {
            assert_same_order(
                Position {
                    board,
                    side_to_move: Color::Black,
                },
                u64::MAX,
            );
        }
    }

    #[test]
    fn lazy_placements_match_eager_order_through_eight_discs() {
        let mut seen = HashSet::new();
        let mut open = vec![Position::initial()];
        while let Some(position) = open.pop() {
            if !seen.insert(position) {
                continue;
            }
            assert_same_order(position, u64::MAX);
            for (next, _) in successors(position) {
                if next.board.popcount() <= 8 {
                    open.push(next);
                }
            }
        }
    }

    #[test]
    fn lazy_placements_match_eager_order_in_full_legal_games() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x6a6e_5eed);
        for _ in 0..16 {
            let mut position = Position::initial();
            loop {
                assert_same_order(position, u64::MAX);
                let next = successors(position);
                if next.is_empty() {
                    break;
                }
                position = next[rng.random_range(0..next.len())].0;
            }
        }
    }
}
