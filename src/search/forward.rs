//! Forward reachability with exact and equivalence-class meeting tables.

pub(crate) mod frontier;

use std::collections::HashSet;
use std::io;

use dashmap::DashSet;
use rayon::prelude::*;

use crate::hash::CustomHash;
use crate::othello::{
    flip, get_moves, validate_board, Board, BoardSymmetry, Color, Direction, Position,
};
use crate::record::{GoalMatch, PlyAction};
use crate::search::moves::{pass_predecessor, PlacementPredecessors};

const SEQUENTIAL_LAYER_LIMIT: usize = 16_384;

/// Reachable positions at every stone count through the meeting boundary.
/// A goal with fewer stones is looked up at its own stone count. Equivalence
/// tables expose the least board in each eight-symmetry orbit.
pub struct ForwardTable {
    start: Position,
    discs: u32,
    target: Option<Position>,
    goal_match: GoalMatch,
    /// `layers[k]` holds positions with `start.popcount() + k` stones.
    layers: Vec<[Vec<Board>; 2]>,
}

impl ForwardTable {
    pub fn new(discs: u32, target: Option<Position>) -> io::Result<Self> {
        Self::from_start(Position::initial(), discs, target)
    }

    pub fn for_equivalence(discs: u32, target: Position) -> io::Result<Self> {
        let start = Position::initial();
        validate_position(start, "start")?;
        validate_position(target, "target")?;
        validate_discs(start, discs)?;
        let discs = bounded_discs(start, discs, Some(target));

        let initial = [vec![representative(start.board)], Vec::new()];
        let layers = build_layers(
            start,
            discs,
            Some(target),
            GoalMatch::EquivalenceClass,
            initial,
        );
        Ok(Self {
            start,
            discs,
            target: Some(target),
            goal_match: GoalMatch::EquivalenceClass,
            layers,
        })
    }

    /// Build an exact meeting table from a validated custom starting position.
    pub fn from_start(start: Position, discs: u32, target: Option<Position>) -> io::Result<Self> {
        validate_position(start, "start")?;
        if let Some(goal) = target {
            validate_position(goal, "target")?;
        }
        validate_discs(start, discs)?;
        let discs = bounded_discs(start, discs, target);

        let mut initial = [Vec::new(), Vec::new()];
        initial[color_index(start.side_to_move)].push(start.board);
        if let Some(passed) = pass_successor(start) {
            if target.is_none_or(|goal| compatible(passed, goal, GoalMatch::Exact)) {
                initial[color_index(passed.side_to_move)].push(passed.board);
            }
        }
        let layers = build_layers(start, discs, target, GoalMatch::Exact, initial);

        Ok(Self {
            start,
            discs,
            target,
            goal_match: GoalMatch::Exact,
            layers,
        })
    }

    /// A target-pruned table proves absence only for the target used to build it.
    pub(crate) fn validate_goal(&self, goal: Position) -> io::Result<()> {
        validate_board(&goal.board).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid goal: {error:?}"),
            )
        })?;
        if self.target.is_some_and(|target| match self.goal_match {
            GoalMatch::Exact => target != goal,
            GoalMatch::EquivalenceClass => target.board != goal.board,
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "forward table was pruned for another goal",
            ));
        }
        Ok(())
    }

    pub fn start(&self) -> Position {
        self.start
    }
    pub fn discs(&self) -> u32 {
        self.discs
    }
    pub fn goal_match(&self) -> GoalMatch {
        self.goal_match
    }
    pub fn contains(&self, position: Position) -> bool {
        let Some(offset) = position
            .board
            .popcount()
            .checked_sub(self.start.board.popcount())
        else {
            return false;
        };
        self.layers.get(offset as usize).is_some_and(|layer| {
            let index = match self.goal_match {
                GoalMatch::Exact => color_index(position.side_to_move),
                GoalMatch::EquivalenceClass => 0,
            };
            let board = match self.goal_match {
                GoalMatch::Exact => position.board,
                GoalMatch::EquivalenceClass => representative(position.board),
            };
            layer[index].binary_search(&board).is_ok()
        })
    }
    pub fn len(&self) -> usize {
        self.layers
            .iter()
            .flat_map(|layer| layer.iter())
            .map(Vec::len)
            .sum()
    }
    pub fn is_empty(&self) -> bool {
        self.layers
            .iter()
            .flat_map(|layer| layer.iter())
            .all(Vec::is_empty)
    }
    /// Iterate over stored positions. Equivalence tables return one canonical
    /// representative per eight-symmetry orbit with a nominal Black turn.
    pub fn positions(&self) -> impl Iterator<Item = Position> + '_ {
        self.layers
            .iter()
            .flat_map(|layer| layer_positions(layer, self.goal_match))
    }

    /// Iterate over the final layer. Equivalence tables return canonical
    /// representatives rather than every geometric image.
    #[cfg(test)]
    pub(crate) fn boundary(&self) -> impl Iterator<Item = Position> + '_ {
        layer_positions(
            self.layers.last().expect("the starting layer is present"),
            self.goal_match,
        )
    }

    pub(crate) fn boundary_layer(&self) -> &[Vec<Board>; 2] {
        self.layers.last().expect("the starting layer is present")
    }

    pub fn actions_to(&self, mut position: Position) -> Option<(Vec<PlyAction>, BoardSymmetry)> {
        if !self.contains(position) {
            return None;
        }
        let placement_candidates = !(self.start.board.player | self.start.board.opponent);
        let mut actions = Vec::new();
        let symmetry = loop {
            if let Some(symmetry) = self.start_symmetry(position) {
                break symmetry;
            }
            let predecessor = pass_predecessor(position)
                .filter(|&previous| self.contains(previous))
                .map(|previous| (previous, PlyAction::Pass))
                .or_else(|| {
                    PlacementPredecessors::new(position, placement_candidates)
                        .find(|(previous, _)| self.contains(*previous))
                });
            let (parent, action) = predecessor?;
            actions.push(action);
            position = parent;
        };
        actions.reverse();
        Some((actions, symmetry))
    }

    fn start_symmetry(&self, position: Position) -> Option<BoardSymmetry> {
        match self.goal_match {
            GoalMatch::Exact => (position == self.start).then_some(BoardSymmetry::Identity),
            GoalMatch::EquivalenceClass => {
                if position.board == self.start.board {
                    Some(BoardSymmetry::Identity)
                } else if position.board == BoardSymmetry::MirrorHorizontal.apply(self.start.board)
                {
                    Some(BoardSymmetry::MirrorHorizontal)
                } else {
                    None
                }
            }
        }
    }
}

fn validate_position(position: Position, name: &str) -> io::Result<()> {
    validate_board(&position.board).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid {name}: {error:?}"),
        )
    })
}

fn validate_discs(start: Position, discs: u32) -> io::Result<()> {
    if !(start.board.popcount()..=64).contains(&discs) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "meeting discs must be between the starting count and 64",
        ));
    }
    Ok(())
}

fn bounded_discs(start: Position, discs: u32, target: Option<Position>) -> u32 {
    target.map_or(discs, |goal| {
        discs.min(goal.board.popcount()).max(start.board.popcount())
    })
}

fn build_layers(
    start: Position,
    discs: u32,
    target: Option<Position>,
    goal_match: GoalMatch,
    initial: [Vec<Board>; 2],
) -> Vec<[Vec<Board>; 2]> {
    let equivalence_targets = match (target, goal_match) {
        (Some(target), GoalMatch::EquivalenceClass) => symmetry_images(target),
        _ => Vec::new(),
    };
    let mut layers = Vec::with_capacity((discs - start.board.popcount() + 1) as usize);
    layers.push(initial);
    while start.board.popcount() + layers.len() as u32 - 1 < discs {
        let next = build_next_layer(
            layers.last().expect("the starting layer is present"),
            target,
            goal_match,
            &equivalence_targets,
            SEQUENTIAL_LAYER_LIMIT,
        );
        layers.push(next);
    }
    layers
}

fn color_index(color: Color) -> usize {
    match color {
        Color::Black => 0,
        Color::White => 1,
    }
}

fn representative(board: Board) -> Board {
    let [player, opponent] = board.unique();
    Board::new(player, opponent)
}

fn symmetry_images(position: Position) -> Vec<Position> {
    let mut images: Vec<_> = BoardSymmetry::ALL
        .into_iter()
        .map(|symmetry| symmetry.apply_position(position))
        .collect();
    images.sort_unstable();
    images.dedup();
    images
}

fn layer_positions(
    layer: &[Vec<Board>; 2],
    goal_match: GoalMatch,
) -> impl Iterator<Item = Position> + '_ {
    [Color::Black, Color::White]
        .into_iter()
        .zip(layer)
        .filter(move |(side_to_move, _)| {
            goal_match == GoalMatch::Exact || *side_to_move == Color::Black
        })
        .flat_map(|(side_to_move, boards)| {
            boards.iter().copied().map(move |board| Position {
                board,
                side_to_move,
            })
        })
}

fn pass_successor(position: Position) -> Option<Position> {
    let Board { player, opponent } = position.board;
    (get_moves(player, opponent) == 0 && get_moves(opponent, player) != 0).then(|| Position {
        board: position.board.swapped(),
        side_to_move: position.side_to_move.opposite(),
    })
}

fn build_next_layer(
    layer: &[Vec<Board>; 2],
    target: Option<Position>,
    goal_match: GoalMatch,
    equivalence_targets: &[Position],
    sequential_layer_limit: usize,
) -> [Vec<Board>; 2] {
    if layer.iter().map(Vec::len).sum::<usize>() <= sequential_layer_limit {
        build_next_layer_sequential(layer, target, goal_match, equivalence_targets)
    } else {
        build_next_layer_parallel(layer, target, goal_match, equivalence_targets)
    }
}

fn build_next_layer_sequential(
    layer: &[Vec<Board>; 2],
    target: Option<Position>,
    goal_match: GoalMatch,
    equivalence_targets: &[Position],
) -> [Vec<Board>; 2] {
    let mut next: [HashSet<Board, CustomHash>; 2] =
        std::array::from_fn(|_| HashSet::with_hasher(CustomHash::default()));
    for (side_index, boards) in layer.iter().enumerate() {
        let side_to_move = if side_index == 0 {
            Color::Black
        } else {
            Color::White
        };
        for &board in boards {
            expand_board(
                board,
                side_to_move,
                target,
                goal_match,
                equivalence_targets,
                |child_index, child_board| {
                    next[child_index].insert(child_board);
                },
            );
        }
    }
    next.map(|set| {
        let mut boards = Vec::with_capacity(set.len());
        boards.extend(set);
        boards.sort_unstable();
        boards
    })
}

fn build_next_layer_parallel(
    layer: &[Vec<Board>; 2],
    target: Option<Position>,
    goal_match: GoalMatch,
    equivalence_targets: &[Position],
) -> [Vec<Board>; 2] {
    let next: [DashSet<Board, CustomHash>; 2] =
        std::array::from_fn(|_| DashSet::with_hasher(CustomHash::default()));
    layer
        .par_iter()
        .enumerate()
        .for_each(|(side_index, boards)| {
            let side_to_move = if side_index == 0 {
                Color::Black
            } else {
                Color::White
            };
            boards.par_iter().for_each(|&board| {
                expand_board(
                    board,
                    side_to_move,
                    target,
                    goal_match,
                    equivalence_targets,
                    |child_index, child_board| {
                        next[child_index].insert(child_board);
                    },
                );
            });
        });
    next.map(|set| {
        let mut boards = Vec::with_capacity(set.len());
        boards.extend(set);
        boards.par_sort_unstable();
        boards
    })
}

fn expand_board(
    board: Board,
    side_to_move: Color,
    target: Option<Position>,
    goal_match: GoalMatch,
    equivalence_targets: &[Position],
    mut insert: impl FnMut(usize, Board),
) {
    let mut moves = get_moves(board.player, board.opponent);
    while moves != 0 {
        let square = moves.trailing_zeros() as usize;
        moves &= moves - 1;
        let flips = flip(square, board.player, board.opponent);
        let child = Position {
            board: Board::new(
                board.opponent & !flips,
                board.player | flips | (1_u64 << square),
            ),
            side_to_move: side_to_move.opposite(),
        };
        let target_compatible = match (target, goal_match) {
            (None, _) => true,
            (Some(goal), GoalMatch::Exact) => compatible(child, goal, GoalMatch::Exact),
            (Some(_), GoalMatch::EquivalenceClass) => equivalence_targets
                .iter()
                .any(|&goal| compatible(child, goal, GoalMatch::EquivalenceClass)),
        };
        if !target_compatible {
            continue;
        }
        for position in std::iter::once(child).chain(pass_successor(child)) {
            let child_index = match goal_match {
                GoalMatch::Exact => color_index(position.side_to_move),
                GoalMatch::EquivalenceClass => 0,
            };
            let child_board = match goal_match {
                GoalMatch::Exact => position.board,
                GoalMatch::EquivalenceClass => representative(position.board),
            };
            insert(child_index, child_board);
        }
    }
}

/// Occupied squares that cannot flip before the target occupancy is reached.
fn stable_discs(occupied: u64, target: u64) -> u64 {
    let mut stable = 0;
    let mut remaining = occupied;
    while remaining != 0 {
        let index = remaining.trailing_zeros();
        remaining &= remaining - 1;
        let (x, y) = ((index % 8) as i32, (index / 8) as i32);
        let mut can_flip = false;
        for direction in Direction::all() {
            let (dx, dy) = direction.to_offset();
            let (mut u, mut v) = (x + dx, y + dy);
            while (0..8).contains(&u) && (0..8).contains(&v) && occupied & (1 << (8 * v + u)) != 0 {
                u += dx;
                v += dy;
            }
            let (other_x, other_y) = (x - dx, y - dy);
            if (0..8).contains(&u)
                && (0..8).contains(&v)
                && target & (1 << (8 * v + u)) != 0
                && (0..8).contains(&other_x)
                && (0..8).contains(&other_y)
                && target & (1 << (8 * other_y + other_x)) != 0
            {
                can_flip = true;
                break;
            }
        }
        if !can_flip {
            stable |= 1 << index;
        }
    }
    stable
}

/// Necessary occupancy and immutable-color conditions for continuing to target.
/// Used both to prune a forward table and to derive a goal-specific frontier.
fn compatible(position: Position, target: Position, goal_match: GoalMatch) -> bool {
    let occupied = position.board.player | position.board.opponent;
    let target_occupied = target.board.player | target.board.opponent;
    if occupied & target_occupied != occupied {
        return false;
    }
    let stable = stable_discs(occupied, target_occupied);
    match goal_match {
        GoalMatch::Exact => {
            let black = |p: Position| {
                if p.side_to_move == Color::Black {
                    p.board.player
                } else {
                    p.board.opponent
                }
            };
            black(position) & stable == black(target) & stable
        }
        GoalMatch::EquivalenceClass => {
            let player = position.board.player & stable;
            player == target.board.player & stable || player == target.board.opponent & stable
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashSet, VecDeque};

    use super::{build_next_layer, compatible, symmetry_images, ForwardTable};
    use crate::io::parse_position;
    use crate::othello::{Board, BoardSymmetry, Color, Position};
    use crate::record::{replay, GoalMatch, PlyAction};
    use crate::search::moves::successors;

    fn naive_positions(start: Position, discs: u32, target: Option<Position>) -> HashSet<Position> {
        let discs = target.map_or(discs, |goal| {
            discs.min(goal.board.popcount()).max(start.board.popcount())
        });
        let mut positions = HashSet::from([start]);
        let mut open = VecDeque::from([start]);
        while let Some(position) = open.pop_front() {
            for (next, _) in successors(position) {
                if next.board.popcount() > discs
                    || target.is_some_and(|goal| !compatible(next, goal, GoalMatch::Exact))
                {
                    continue;
                }
                if positions.insert(next) {
                    open.push_back(next);
                }
            }
        }
        positions
    }

    fn assert_matches_naive(table: &ForwardTable, expected: &HashSet<Position>) {
        let actual: HashSet<_> = table.positions().collect();
        assert_eq!(actual, *expected);
        assert_eq!(table.len(), expected.len());
        assert_eq!(table.is_empty(), expected.is_empty());
        for &position in expected {
            assert!(table.contains(position));
        }
    }

    fn assert_actions_reach(table: &ForwardTable, goal: Position) {
        let (actions, symmetry) = table
            .actions_to(goal)
            .expect("every table position must have a forward witness");
        assert_eq!(symmetry, BoardSymmetry::Identity);
        let trace = replay(table.start(), &actions).unwrap();
        assert_eq!(trace.last(), Some(&goal));
    }

    fn forced_pass_trace() -> Vec<Position> {
        let start =
            parse_position("OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO w")
                .unwrap();
        replay(
            start,
            &[PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)],
        )
        .unwrap()
    }

    fn assert_next_layer_paths_match(
        layer: &[Vec<Board>; 2],
        target: Option<Position>,
        goal_match: GoalMatch,
    ) {
        let parent_count = layer.iter().map(Vec::len).sum::<usize>();
        assert!(parent_count > 0);
        let equivalence_targets = match (target, goal_match) {
            (Some(target), GoalMatch::EquivalenceClass) => symmetry_images(target),
            _ => Vec::new(),
        };
        let sequential = build_next_layer(
            layer,
            target,
            goal_match,
            &equivalence_targets,
            parent_count,
        );
        let parallel = build_next_layer(
            layer,
            target,
            goal_match,
            &equivalence_targets,
            parent_count - 1,
        );

        assert_eq!(sequential, parallel);
    }

    #[test]
    fn sequential_and_parallel_next_layers_match_for_all_modes() {
        let target_table = ForwardTable::new(9, None).unwrap();
        let target = target_table.boundary().next().unwrap();
        let exact = ForwardTable::new(8, None).unwrap();
        let equivalence = ForwardTable::for_equivalence(8, target).unwrap();

        for (layer, goal_match) in [
            (exact.boundary_layer(), GoalMatch::Exact),
            (equivalence.boundary_layer(), GoalMatch::EquivalenceClass),
        ] {
            for target in [None, Some(target)] {
                assert_next_layer_paths_match(layer, target, goal_match);
            }
        }
    }

    #[test]
    fn target_pruned_layers_match_naive_bfs_for_representative_goals() {
        let start = Position::initial();
        let reachable = naive_positions(start, 8, None);
        let mut boundary: Vec<_> = reachable
            .iter()
            .copied()
            .filter(|position| position.board.popcount() == 8)
            .collect();
        boundary.sort_unstable();
        let shallow = reachable
            .iter()
            .copied()
            .find(|position| position.board.popcount() == 6)
            .unwrap();
        let mut wrong_turn = boundary[boundary.len() / 2];
        wrong_turn.side_to_move = wrong_turn.side_to_move.opposite();
        let targets = [
            shallow,
            boundary[0],
            boundary[boundary.len() / 3],
            boundary[boundary.len() * 2 / 3],
            boundary[boundary.len() - 1],
            wrong_turn,
        ];

        for target in targets {
            let expected = naive_positions(start, 8, Some(target));
            let table = ForwardTable::new(8, Some(target)).unwrap();
            assert_matches_naive(&table, &expected);
        }
    }

    #[test]
    fn forced_pass_and_terminal_positions_survive_each_boundary() {
        let trace = forced_pass_trace();
        let start = trace[0];

        for target in [None, Some(trace[2])] {
            let table = ForwardTable::from_start(start, 63, target).unwrap();
            assert!(table.contains(trace[1]));
            assert!(table.contains(trace[2]));
            assert!(!table.contains(trace[3]));
            assert_actions_reach(&table, trace[1]);
            assert_actions_reach(&table, trace[2]);
        }
        for target in [None, Some(trace[3])] {
            let table = ForwardTable::from_start(start, 64, target).unwrap();
            assert!(table.contains(trace[1]));
            assert!(table.contains(trace[2]));
            assert!(table.contains(trace[3]));
            assert_actions_reach(&table, trace[3]);
        }
    }

    #[test]
    fn incompatible_initial_pass_is_pruned_but_the_start_is_retained() {
        let trace = forced_pass_trace();
        let start = trace[1];
        assert_eq!(successors(start), [(trace[2], PlyAction::Pass)]);
        let target = Position {
            side_to_move: start.side_to_move.opposite(),
            ..start
        };
        assert!(!compatible(trace[2], target, GoalMatch::Exact));

        let table = ForwardTable::from_start(start, 63, Some(target)).unwrap();
        assert_eq!(table.positions().collect::<Vec<_>>(), [start]);
        assert!(table.contains(start));
        assert!(!table.contains(trace[2]));
        assert_eq!(
            table.actions_to(start),
            Some((Vec::new(), BoardSymmetry::Identity))
        );
    }

    #[test]
    fn target_below_a_custom_start_keeps_the_start_as_the_boundary() {
        let target = Position::initial();
        let start = successors(target)[0].0;
        let table = ForwardTable::from_start(start, 6, Some(target)).unwrap();

        assert_eq!(table.discs(), start.board.popcount());
        assert!(table.contains(start));
        assert!(!table.contains(target));
    }

    #[test]
    fn equivalence_table_contains_both_relative_openings_and_ignores_turn() {
        let start = Position::initial();
        let mirrored = BoardSymmetry::MirrorHorizontal.apply_position(start);
        let target = successors(start)[0].0;
        let table = ForwardTable::for_equivalence(4, target).unwrap();

        assert_eq!(table.goal_match(), GoalMatch::EquivalenceClass);
        assert_eq!(table.len(), 1);
        for (position, expected_symmetry) in [
            (start, BoardSymmetry::Identity),
            (mirrored, BoardSymmetry::MirrorHorizontal),
        ] {
            for side_to_move in [Color::Black, Color::White] {
                let position = Position {
                    side_to_move,
                    ..position
                };
                assert!(table.contains(position));
                assert_eq!(
                    table.actions_to(position),
                    Some((Vec::new(), expected_symmetry))
                );
            }
        }
        assert!(table
            .positions()
            .all(|position| position.side_to_move == Color::Black));

        table
            .validate_goal(Position {
                side_to_move: target.side_to_move.opposite(),
                ..target
            })
            .unwrap();
        assert!(table.validate_goal(start).is_err());
    }
}
