use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::io;
use std::sync::{Condvar, Mutex};

use crate::filters::{
    impossible_edges::check_virtual_edge_patterns, linear_programming::check_lp, FilterResult,
};
use crate::othello::{get_last_moves, Position};
use crate::record::PlyAction;
use crate::search::{
    constraints::ReverseConstraints,
    core::{found, pass_predecessor, PlacementPredecessors, SearchNode, SearchOutcome},
    forward::ForwardTable,
    transposition::Visited,
};

/// Count exactly the placement predecessors, without materializing them.
///
/// In a direction with `length` consecutive stones of the last player, an
/// undo may flip 0 through `length - 1` stones: one must remain as the bounding
/// stone. Empty rays allow only zero. The rays are disjoint, so their choices
/// multiply, and the all-zero choice is not a legal placement. `get_last_moves`
/// excludes squares with an additional capture in an unchanged direction.
fn placement_predecessor_count(position: Position) -> usize {
    let board = position.board;
    let mut squares = get_last_moves(board.opponent, board.player);
    let mut total = 0;
    while squares != 0 {
        let square = squares.trailing_zeros() as i32;
        squares &= squares - 1;
        let mut combinations = 1;
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
            let (mut x, mut y) = (square % 8 + dx, square / 8 + dy);
            let mut length = 0;
            while (0..8).contains(&x)
                && (0..8).contains(&y)
                && board.opponent & (1_u64 << (8 * y + x)) != 0
            {
                length += 1;
                x += dx;
                y += dy;
            }
            combinations *= length.max(1);
        }
        total += combinations - 1;
    }
    total
}

fn heuristic_function(position: Position) -> usize {
    let branching = placement_predecessor_count(position);
    if branching == 0 {
        usize::MAX // Infinity for this bounded integer priority: dead ends go last.
    } else {
        branching
    }
}

struct Entry {
    priority: usize,
    node: SearchNode,
    // Cached before registration so expansion does not repeat the necessary
    // conditions. A rejected root retains None for node-limit accounting.
    candidates: Option<u64>,
}
impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.node.position == other.node.position
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .priority
            .cmp(&self.priority)
            .then_with(|| other.node.position.cmp(&self.node.position))
    }
}

struct Work {
    open: BinaryHeap<Entry>,
    visited: Visited,
    active: usize,
    /// Distinct registered positions, including both possible initial states.
    nodes: usize,
    result: Option<io::Result<SearchOutcome>>,
}

impl Work {
    /// Removing a task and counting it as active are one locked operation.
    fn take(&mut self) -> Option<Entry> {
        let entry = self.open.pop()?;
        self.active += 1;
        Some(entry)
    }

    fn exhausted(&self) -> bool {
        self.open.is_empty() && self.active == 0
    }
}

struct Shared {
    work: Mutex<Work>,
    changed: Condvar,
}
impl Shared {
    fn claim(&self, node_limit: usize) -> Option<Entry> {
        let mut work = self.work.lock().expect("search mutex poisoned");
        loop {
            if work.result.is_some() {
                return None;
            }
            // The original bound applies before removing another state,
            // including before its meeting-table lookup.
            if work.nodes >= node_limit {
                work.result = Some(Ok(SearchOutcome::Unknown { nodes: work.nodes }));
                self.changed.notify_all();
                return None;
            }
            if let Some(node) = work.take() {
                return Some(node);
            }
            if work.exhausted() {
                work.result = Some(Ok(SearchOutcome::NotFound { nodes: work.nodes }));
                self.changed.notify_all();
                return None;
            }
            work = self.changed.wait(work).expect("search mutex poisoned");
        }
    }
}

struct Context<'a> {
    table: &'a ForwardTable,
    constraints: ReverseConstraints,
    standard_start: bool,
}

impl Context<'_> {
    fn prepare(&self, node: SearchNode, root: bool) -> Option<Entry> {
        let position = node.position;
        let meeting = self.table.contains(position);
        let candidates = if meeting {
            Some(0)
        } else if position.board.popcount() <= self.table.discs()
            || self.standard_start
                && !check_virtual_edge_patterns(position.board.player, position.board.opponent)
        {
            None
        } else {
            self.constraints.candidates(position)
        };
        if candidates.is_none() && !root {
            return None;
        }
        // A reverse-pass state is prepared separately by the caller. A
        // nonmeeting state with no placement predecessor has no other work.
        if !root && !meeting && candidates == Some(0) {
            return None;
        }
        Some(Entry {
            priority: if meeting {
                0
            } else {
                heuristic_function(position)
            },
            node,
            candidates,
        })
    }
}

fn expand_node(
    node: &SearchNode,
    candidates: u64,
    context: &Context<'_>,
    shared: &Shared,
    node_limit: usize,
) {
    let previous = PlacementPredecessors::new(node.position, candidates);
    for (position, action) in previous {
        let child = node.preceding(position, action);
        // A frontier step removes one placement. Its predecessor before a
        // legal pass is also registered in that same stone-count layer; the
        // certificate still stores the placement and pass as separate actions.
        let before_pass =
            pass_predecessor(position).map(|previous| child.preceding(previous, PlyAction::Pass));
        for child in std::iter::once(child).chain(before_pass) {
            {
                let work = shared.work.lock().expect("search mutex poisoned");
                if work.result.is_some() {
                    return;
                }
                if work.visited.contains(&child.position) {
                    continue;
                }
            }
            let Some(entry) = context.prepare(child, false) else {
                continue;
            };
            let mut work = shared.work.lock().expect("search mutex poisoned");
            if work.result.is_some() {
                return;
            }
            if work.visited.insert(entry.node.position) {
                work.nodes += 1;
                if work.nodes > node_limit {
                    work.result = Some(Ok(SearchOutcome::Unknown { nodes: work.nodes }));
                    shared.changed.notify_all();
                    return;
                }
                work.open.push(entry);
                shared.changed.notify_one();
            }
        }
    }
}

/// Greedy best-first search ranks nonzero placement-branch counts ascending,
/// with zero last and no contribution from passes. Queue and in-flight
/// accounting share a mutex, including exhaustion. The node limit counts
/// distinct registered positions, including the initial state and reverse pass.
pub fn parallel_retrospective_greedy_best_first_search(
    goal: Position,
    table: &ForwardTable,
    node_limit: usize,
    use_lp: bool,
    threads: usize,
) -> io::Result<SearchOutcome> {
    table.validate_goal(goal)?;
    if table.contains(goal) {
        return found(table, goal, &SearchNode::root(goal), 0);
    }
    if goal.board.popcount() <= table.discs() {
        return Ok(SearchOutcome::NotFound { nodes: 0 });
    }
    let context = Context {
        table,
        constraints: ReverseConstraints::new(table, goal),
        standard_start: table.start() == Position::initial(),
    };
    let workers = if threads == 0 { 1 } else { threads };
    let mut open = BinaryHeap::new();
    let root = SearchNode::root(goal);
    let before_pass =
        pass_predecessor(goal).map(|previous| root.preceding(previous, PlyAction::Pass));
    let mut visited = Visited::default();
    for node in std::iter::once(root).chain(before_pass) {
        if visited.insert(node.position) {
            open.push(
                context
                    .prepare(node, true)
                    .expect("root registration is unconditional"),
            );
        }
    }
    let nodes = visited.len();
    let shared = Shared {
        work: Mutex::new(Work {
            open,
            visited,
            active: 0,
            nodes,
            result: None,
        }),
        changed: Condvar::new(),
    };
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while let Some(entry) = shared.claim(node_limit) {
                    let node = entry.node;
                    let expansion = (|| -> io::Result<()> {
                        if table.contains(node.position) {
                            let mut work = shared.work.lock().expect("search mutex poisoned");
                            if !matches!(work.result, Some(Err(_))) {
                                work.result = Some(found(table, goal, &node, work.nodes));
                            }
                            return Ok(());
                        }
                        if node.position.board.popcount() <= table.discs() {
                            return Ok(());
                        }
                        let Some(candidates) = entry.candidates else {
                            return Ok(());
                        };
                        if use_lp && context.standard_start {
                            let board = node.position.board;
                            let result = check_lp(board.player, board.opponent, false, false)?;
                            if let FilterResult::Undetermined { reason } = &result {
                                eprintln!(
                                    "info: LP undetermined for {}: {}; retaining this search node",
                                    crate::io::format_position(node.position),
                                    reason
                                );
                            }
                            if !result.passes() {
                                return Ok(());
                            }
                        }
                        expand_node(&node, candidates, &context, &shared, node_limit);
                        Ok(())
                    })();
                    let mut work = shared.work.lock().expect("search mutex poisoned");
                    work.active -= 1;
                    if let Err(error) = expansion {
                        work.result = Some(Err(error));
                    }
                    shared.changed.notify_all();
                }
            });
        }
    });
    shared
        .work
        .into_inner()
        .expect("search mutex poisoned")
        .result
        .ok_or_else(|| io::Error::other("workers stopped without a result"))?
}

#[cfg(test)]
mod tests {
    use super::{
        heuristic_function, parallel_retrospective_greedy_best_first_search,
        placement_predecessor_count, Context,
    };
    use crate::filters::{impossible_edges::check_virtual_edge_patterns, reverse::PlacementFilter};
    use crate::othello::{Board, Color, Position, CENTER_MASK};
    use crate::record::PlyAction;
    use crate::search::{
        constraints::ReverseConstraints,
        core::{
            append_placement_predecessors, pass_predecessor, successors, PlacementPredecessors,
            SearchNode, SearchOutcome,
        },
        forward::ForwardTable,
        frontier::ForwardOccupancyIndex,
    };
    use rand::{Rng, SeedableRng};

    fn assert_count_matches_enumeration(position: Position) {
        let mut previous = Vec::new();
        append_placement_predecessors(position, u64::MAX, &mut previous);
        assert_eq!(
            placement_predecessor_count(position),
            previous.len(),
            "position={position:?}"
        );
        assert_eq!(
            heuristic_function(position),
            if previous.is_empty() {
                usize::MAX
            } else {
                previous.len()
            }
        );
    }

    #[test]
    fn predecessor_count_matches_legal_playouts() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x6762_6673);
        for _ in 0..16 {
            let mut position = Position::initial();
            loop {
                assert_count_matches_enumeration(position);
                let next = successors(position);
                if next.is_empty() {
                    break;
                }
                position = next[rng.random_range(0..next.len())].0;
            }
        }
    }

    #[test]
    fn predecessor_count_matches_sparse_and_dense_boards() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x7261_7973);
        for _ in 0..256 {
            let a = rng.random::<u64>();
            let b = rng.random::<u64>();
            for occupied in [a & b, a, a | b, u64::MAX] {
                let occupied = occupied | CENTER_MASK;
                let player = rng.random::<u64>() & occupied;
                for side_to_move in [Color::Black, Color::White] {
                    assert_count_matches_enumeration(Position {
                        board: Board::new(player, occupied ^ player),
                        side_to_move,
                    });
                }
            }
        }
        // Long same-color rays exercise products of several undo directions,
        // including terminal positions and the no-predecessor case.
        for opponent in [0, CENTER_MASK, 0xff00_0000_0000_00ff, u64::MAX] {
            assert_count_matches_enumeration(Position {
                board: Board::new(CENTER_MASK & !opponent, opponent),
                side_to_move: Color::Black,
            });
        }
    }

    #[test]
    fn predecessor_count_matches_rays_from_every_square() {
        for square in 0..64 {
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
                let (mut x, mut y) = (square % 8, square / 8);
                let mut opponent = 0;
                while (0..8).contains(&x) && (0..8).contains(&y) {
                    opponent |= 1_u64 << (8 * y + x);
                    assert_count_matches_enumeration(Position {
                        board: Board::new(CENTER_MASK & !opponent, opponent),
                        side_to_move: Color::Black,
                    });
                    x += dx;
                    y += dy;
                }
            }
        }
    }

    fn assert_found(outcome: SearchOutcome, expected_nodes: Option<usize>, actions: &[PlyAction]) {
        let SearchOutcome::Found { record, nodes } = outcome else {
            panic!("expected FOUND, got {outcome:?}");
        };
        if let Some(expected_nodes) = expected_nodes {
            assert_eq!(nodes, expected_nodes);
        }
        assert_eq!(record.actions, actions);
        assert_eq!(record.verify().unwrap().last(), Some(&record.goal));
    }

    fn shallow_pass_positions() -> (Position, Position, Position, Position) {
        let start = Position {
            board: Board::new(0x0000_0000_0001_0f1f, 0x0000_0038_1c0e_1020),
            side_to_move: Color::White,
        };
        let before_pass = Position {
            board: Board::new(0x0000_0038_1c0e_1000, 0x0000_0000_0001_0f7f),
            side_to_move: Color::Black,
        };
        let after_pass = Position {
            board: Board::new(0x0000_0000_0001_0f7f, 0x0000_0038_1c0e_1000),
            side_to_move: Color::White,
        };
        let child_goal = Position {
            board: Board::new(0x0000_0038_1c0e_0000, 0x0000_0000_0001_3f7f),
            side_to_move: Color::Black,
        };
        assert!(successors(start).contains(&(before_pass, PlyAction::Play(6))));
        assert_eq!(successors(before_pass), [(after_pass, PlyAction::Pass)]);
        assert!(successors(after_pass).contains(&(child_goal, PlyAction::Play(13))));
        assert_eq!(pass_predecessor(after_pass), Some(before_pass));
        (start, before_pass, after_pass, child_goal)
    }

    #[test]
    fn boundary_decisions_precede_zero_and_one_node_limits() {
        let table = ForwardTable::new(4, None).unwrap();
        let initial = Position::initial();
        let unavailable = Position {
            side_to_move: Color::White,
            ..initial
        };
        assert!(!table.contains(unavailable));
        for node_limit in [0, 1] {
            assert_found(
                parallel_retrospective_greedy_best_first_search(
                    initial, &table, node_limit, false, 3,
                )
                .unwrap(),
                Some(0),
                &[],
            );
            assert!(matches!(
                parallel_retrospective_greedy_best_first_search(
                    unavailable,
                    &table,
                    node_limit,
                    false,
                    3,
                )
                .unwrap(),
                SearchOutcome::NotFound { nodes: 0 }
            ));
        }
    }

    #[test]
    fn root_reverse_pass_reaches_the_forward_frontier() {
        let (start, _, goal, _) = shallow_pass_positions();
        let table = ForwardTable::from_start(start, 21, Some(goal)).unwrap();
        assert!(!table.contains(goal));
        assert!(pass_predecessor(goal).is_some());
        assert!(matches!(
            parallel_retrospective_greedy_best_first_search(goal, &table, 2, false, 1).unwrap(),
            SearchOutcome::Unknown { nodes: 2 }
        ));
        assert_found(
            parallel_retrospective_greedy_best_first_search(goal, &table, usize::MAX, false, 1)
                .unwrap(),
            None,
            &[PlyAction::Play(6), PlyAction::Pass],
        );
    }

    #[test]
    fn placement_child_registers_its_reverse_pass() {
        let (start, _, _, goal) = shallow_pass_positions();
        let table = ForwardTable::from_start(start, 21, Some(goal)).unwrap();
        assert_found(
            parallel_retrospective_greedy_best_first_search(goal, &table, usize::MAX, false, 1)
                .unwrap(),
            None,
            &[PlyAction::Play(6), PlyAction::Pass, PlyAction::Play(13)],
        );
    }

    #[test]
    fn forward_boundary_retains_a_pass_witness() {
        let (_, start, goal, _) = shallow_pass_positions();
        let table = ForwardTable::from_start(start, 22, Some(goal)).unwrap();
        assert!(table.contains(goal));
        assert_found(
            parallel_retrospective_greedy_best_first_search(goal, &table, 0, false, 3).unwrap(),
            Some(0),
            &[PlyAction::Pass],
        );
    }

    #[test]
    fn zero_placement_candidates_still_register_a_child_pass() {
        let start = Position {
            board: Board::new(0x3f38_bcf6_fa8c_e6fe, 0x40c7_4309_0573_1900),
            side_to_move: Color::Black,
        };
        let before_pass = Position {
            board: Board::new(0x40c6_4208_0472_1800, 0x3f39_bdf7_fb8d_e7ff),
            side_to_move: Color::White,
        };
        let after_pass = Position {
            board: Board::new(0x3f39_bdf7_fb8d_e7ff, 0x40c6_4208_0472_1800),
            side_to_move: Color::Black,
        };
        let goal = Position {
            board: Board::new(0x0006_4208_0472_1800, 0xfff9_bdf7_fb8d_e7ff),
            side_to_move: Color::White,
        };
        assert!(successors(start).contains(&(before_pass, PlyAction::Play(0))));
        assert_eq!(successors(before_pass), [(after_pass, PlyAction::Pass)]);
        assert_eq!(successors(after_pass), [(goal, PlyAction::Play(63))]);
        let table = ForwardTable::from_start(start, 62, Some(goal)).unwrap();
        assert_eq!(
            ReverseConstraints::new(&table, goal).candidates(after_pass),
            Some(0)
        );
        let context = Context {
            table: &table,
            constraints: ReverseConstraints::new(&table, goal),
            standard_start: false,
        };
        assert!(context
            .prepare(SearchNode::root(after_pass), false)
            .is_none());
        assert_eq!(
            context
                .prepare(SearchNode::root(after_pass), true)
                .expect("root registration is unconditional")
                .candidates,
            Some(0)
        );
        assert_found(
            parallel_retrospective_greedy_best_first_search(goal, &table, usize::MAX, false, 1)
                .unwrap(),
            None,
            &[PlyAction::Play(0), PlyAction::Pass, PlyAction::Play(63)],
        );
    }

    #[test]
    fn pruned_children_do_not_consume_the_node_limit() {
        let goal = Position {
            board: Board::new(0x0000_2010_0808_0800, 0x0000_000e_1000_0000),
            side_to_move: Color::White,
        };
        let child = Position {
            board: Board::new(0x0000_0008_1000_0000, 0x0000_2014_0808_0800),
            side_to_move: Color::Black,
        };
        let table = ForwardTable::new(6, None).unwrap();
        let frontier = ForwardOccupancyIndex::new(&table, goal);
        let placement_filter = PlacementFilter::new();
        let occupied = goal.board.player | goal.board.opponent;
        assert_eq!(
            placement_filter.candidates(goal),
            Some(0x0000_0002_0000_0000)
        );
        assert_eq!(frontier.required_squares(occupied), Some(0x001c_1808_0000));
        assert!(check_virtual_edge_patterns(
            goal.board.player,
            goal.board.opponent
        ));
        let candidates = ReverseConstraints::new(&table, goal)
            .candidates(goal)
            .expect("root passes the shared constraints");
        assert_eq!(candidates, 0x0000_0002_0000_0000);
        let previous: Vec<_> = PlacementPredecessors::new(goal, candidates).collect();
        assert_eq!(previous, [(child, PlyAction::Play(33))]);
        assert_eq!(placement_filter.candidates(child), None);
        assert!(frontier
            .required_squares(child.board.player | child.board.opponent)
            .is_some());
        assert!(check_virtual_edge_patterns(
            child.board.player,
            child.board.opponent
        ));
        assert_eq!(pass_predecessor(child), None);

        assert!(matches!(
            parallel_retrospective_greedy_best_first_search(goal, &table, 2, false, 3).unwrap(),
            SearchOutcome::NotFound { nodes: 1 }
        ));
    }

    #[test]
    fn rejected_root_completes_parallel_exhaustion() {
        let goal = Position {
            board: Board::new(CENTER_MASK, (1_u64 << 0) | (1_u64 << 63)),
            side_to_move: Color::Black,
        };
        let table = ForwardTable::new(5, None).unwrap();
        let frontier = ForwardOccupancyIndex::new(&table, goal);
        assert_eq!(
            frontier.required_squares(goal.board.player | goal.board.opponent),
            None
        );
        assert_eq!(ReverseConstraints::new(&table, goal).candidates(goal), None);
        assert!(matches!(
            parallel_retrospective_greedy_best_first_search(goal, &table, 1, false, 3).unwrap(),
            SearchOutcome::Unknown { nodes: 1 }
        ));
        assert!(matches!(
            parallel_retrospective_greedy_best_first_search(goal, &table, 2, false, 3).unwrap(),
            SearchOutcome::NotFound { nodes: 1 }
        ));
    }

    #[test]
    fn opening_child_obeys_registered_node_limits() {
        let table = ForwardTable::new(4, None).unwrap();
        let initial = Position::initial();
        let (goal, action) = successors(initial)[0];
        assert!(table.contains(initial));
        assert!(!table.contains(goal));
        for node_limit in [0, 1] {
            assert!(matches!(
                parallel_retrospective_greedy_best_first_search(
                    goal,
                    &table,
                    node_limit,
                    false,
                    1,
                )
                .unwrap(),
                SearchOutcome::Unknown { nodes: 1 }
            ));
        }
        assert!(matches!(
            parallel_retrospective_greedy_best_first_search(goal, &table, 2, false, 1).unwrap(),
            SearchOutcome::Unknown { nodes: 2 }
        ));
        assert_found(
            parallel_retrospective_greedy_best_first_search(goal, &table, 3, false, 1).unwrap(),
            Some(2),
            &[action],
        );
    }

    #[test]
    fn exact_meetings_precede_heuristic_entries() {
        let table = ForwardTable::new(4, None).unwrap();
        let initial = Position::initial();
        let goal = successors(initial)[0].0;
        let context = Context {
            table: &table,
            constraints: ReverseConstraints::new(&table, goal),
            standard_start: true,
        };
        let meeting = context
            .prepare(SearchNode::root(initial), false)
            .expect("the initial position is in the table");
        let heuristic = context
            .prepare(SearchNode::root(goal), true)
            .expect("root registration is unconditional");
        assert_eq!(meeting.priority, 0);
        assert!(heuristic.priority > meeting.priority);
        let mut open = std::collections::BinaryHeap::from([heuristic, meeting]);
        assert_eq!(open.pop().unwrap().node.position, initial);
    }
}
