use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::io;
use std::sync::{Condvar, Mutex};

use crate::othello::{get_last_moves, Position};
use crate::filters::{
    impossible_edges::check_edge_patterns, linear_programming::check_lp,
    occupancy::check_occupancy, seg3::check_seg3_more, FilterResult,
};
use crate::record::PlyAction;
use crate::search::{
    core::{append_placement_predecessors, found, pass_predecessor, SearchNode, SearchOutcome},
    forward::ForwardTable,
    transposition::Visited,
};

fn heuristic_function(position: Position) -> usize {
    // Placement predecessors retain a bounding stone in each flipped ray.
    // Their replay check rejects exactly the extra captures excluded by the
    // retrospective-flip precheck, so this is its nonempty combination count.
    let mut previous = Vec::new();
    append_placement_predecessors(position, u64::MAX, &mut previous);
    let branching = previous.len();
    if branching == 0 {
        usize::MAX // Infinity for this bounded integer priority: dead ends go last.
    } else {
        branching
    }
}

struct Entry {
    priority: usize,
    node: SearchNode,
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
    fn take(&mut self) -> Option<SearchNode> {
        let entry = self.open.pop()?;
        self.active += 1;
        Some(entry.node)
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
    fn claim(&self, node_limit: usize) -> Option<SearchNode> {
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

fn expand_node(node: &SearchNode, shared: &Shared, node_limit: usize) {
    let board = node.position.board;
    let mut previous = Vec::new();
    append_placement_predecessors(
        node.position,
        get_last_moves(board.opponent, board.player),
        &mut previous,
    );
    for (position, action) in previous {
        let child = node.preceding(position, action);
        // A frontier step removes one placement. Its predecessor before a
        // legal pass is also registered in that same stone-count layer; the
        // certificate still stores the placement and pass as separate actions.
        let before_pass =
            pass_predecessor(position).map(|previous| child.preceding(previous, PlyAction::Pass));
        for child in std::iter::once(child).chain(before_pass) {
            if shared
                .work
                .lock()
                .expect("search mutex poisoned")
                .result
                .is_some()
            {
                return;
            }
            let board = child.position.board;
            if !check_occupancy(board.player | board.opponent)
                || !check_seg3_more(board.player, board.opponent, false)
                || !check_edge_patterns(board.player, board.opponent)
            {
                continue;
            }
            let mut work = shared.work.lock().expect("search mutex poisoned");
            if work.result.is_some() {
                return;
            }
            if work.visited.insert(child.position) {
                work.nodes += 1;
                if work.nodes > node_limit {
                    work.result = Some(Ok(SearchOutcome::Unknown { nodes: work.nodes }));
                    shared.changed.notify_all();
                    return;
                }
                work.open.push(Entry {
                    priority: heuristic_function(child.position),
                    node: child,
                });
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
    let workers = if threads == 0 { 1 } else { threads };
    let mut open = BinaryHeap::new();
    let root = SearchNode::root(goal);
    let before_pass =
        pass_predecessor(goal).map(|previous| root.preceding(previous, PlyAction::Pass));
    let mut visited = Visited::default();
    for node in std::iter::once(root).chain(before_pass) {
        if visited.insert(node.position) {
            open.push(Entry {
                priority: heuristic_function(node.position),
                node,
            });
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
                while let Some(node) = shared.claim(node_limit) {
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
                        if use_lp {
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
                        expand_node(&node, &shared, node_limit);
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
