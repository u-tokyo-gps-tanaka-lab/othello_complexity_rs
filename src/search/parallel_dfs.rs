use std::io;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

use ordered_float::OrderedFloat;

use crate::othello::Position;
use crate::record::PlyAction;
use crate::search::{
    constraints::ReverseConstraints,
    core::{append_placement_predecessors, found, pass_predecessor, SearchNode, SearchOutcome},
    forward::ForwardTable,
    frontier::ForwardOccupancyIndex,
    move_ordering::h_function,
    transposition::ConcurrentVisited,
};

struct PathLink<'a> {
    parent: Option<&'a PathLink<'a>>,
    action: PlyAction,
}

struct Context<'a, 'b> {
    table: &'a ForwardTable,
    constraints: Vec<ReverseConstraints<'b>>,
    node_limit: usize,
    table_limit: usize,
    visited: ConcurrentVisited,
    table_attempts: AtomicUsize,
    nodes: AtomicUsize,
    limited: AtomicBool,
    stopped: AtomicBool,
}

fn order_children(children: &mut [(Position, PlyAction)]) {
    // Stable sorting preserves the established generation order on score ties.
    children.sort_by_cached_key(|(position, _)| OrderedFloat(h_function(&position.board)));
}

fn meeting_node<'a>(position: Position, mut path: Option<&'a PathLink<'a>>) -> SearchNode {
    let mut reverse_actions = Vec::new();
    while let Some(link) = path {
        reverse_actions.push(link.action);
        path = link.parent;
    }
    reverse_actions.reverse();
    SearchNode::from_reverse_actions(position, reverse_actions)
}

impl Context<'_, '_> {
    fn constraints(&self) -> &ReverseConstraints<'_> {
        let index = rayon::current_thread_index().unwrap_or(self.constraints.len() - 1);
        &self.constraints[index]
    }

    fn visit<'a>(
        &self,
        position: Position,
        path: Option<&'a PathLink<'a>>,
        depth: usize,
    ) -> io::Result<Option<SearchNode>> {
        if self.stopped.load(Ordering::Acquire) {
            return Ok(None);
        }
        // A task that has already started can still supply an actual witness
        // while another task reaches the resource limit.
        if self.table.contains(position) {
            let node = meeting_node(position, path);
            self.stopped.store(true, Ordering::Release);
            return Ok(Some(node));
        }
        if position.board.popcount() <= self.table.discs() {
            return Ok(None);
        }

        let table_attempts = self.table_attempts.fetch_add(1, Ordering::AcqRel) + 1;
        if table_attempts < self.table_limit {
            if !self.visited.insert(position) {
                return Ok(None);
            }
        } else if self.visited.contains(&position) {
            return Ok(None);
        }

        let nodes = self.nodes.fetch_add(1, Ordering::AcqRel) + 1;
        if nodes > self.node_limit {
            self.limited.store(true, Ordering::Release);
            self.stopped.store(true, Ordering::Release);
            return Ok(None);
        }

        let Some(candidates) = self.constraints().candidates(position) else {
            return Ok(None);
        };
        let mut previous = Vec::new();
        if let Some(previous_position) = pass_predecessor(position) {
            previous.push((previous_position, PlyAction::Pass));
        }
        append_placement_predecessors(position, candidates, &mut previous);
        order_children(&mut previous);
        if depth < 12 && previous.len() >= 4 {
            let mut children = previous.into_iter();
            let (position, action) = children.next().expect("split has at least four children");
            // Finish the best-ranked branch before issuing the remaining FIFO
            // tasks, as in the original dynamic parallel DFS.
            let child_path = PathLink {
                parent: path,
                action,
            };
            let first = self.visit(position, Some(&child_path), depth + 1)?;
            if first.is_some() || self.stopped.load(Ordering::Acquire) {
                return Ok(first);
            }
            let joined = Mutex::new(Ok(None));
            rayon::scope_fifo(|scope| {
                for (position, action) in children {
                    if self.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let joined = &joined;
                    scope.spawn_fifo(move |_| {
                        let child_path = PathLink {
                            parent: path,
                            action,
                        };
                        let outcome = self.visit(position, Some(&child_path), depth + 1);
                        let mut joined = joined.lock().expect("search mutex poisoned");
                        match outcome {
                            Err(error) => {
                                *joined = Err(error);
                                self.stopped.store(true, Ordering::Release);
                            }
                            Ok(Some(meeting)) if matches!(*joined, Ok(None)) => {
                                *joined = Ok(Some(meeting));
                            }
                            Ok(_) => {}
                        }
                    });
                }
            });
            joined.into_inner().expect("search mutex poisoned")
        } else {
            for (position, action) in previous {
                let child_path = PathLink {
                    parent: path,
                    action,
                };
                if let Some(meeting) = self.visit(position, Some(&child_path), depth + 1)? {
                    return Ok(Some(meeting));
                }
                if self.stopped.load(Ordering::Acquire) {
                    break;
                }
            }
            Ok(None)
        }
    }
}

/// Parallel depth-first exploration. Memoization is registered only while the
/// nonboundary visit-attempt count is strictly below the table bound. The node
/// bound counts visits not suppressed by that table and stops with UNKNOWN.
/// Parallel tasks run on the caller's current Rayon thread pool.
pub fn retrospective_search_parallel(
    goal: Position,
    table: &ForwardTable,
    node_limit: usize,
    table_limit: usize,
) -> io::Result<SearchOutcome> {
    table.validate_goal(goal)?;
    if table.contains(goal) {
        return found(table, goal, &SearchNode::root(goal), 0);
    }
    if goal.board.popcount() <= table.discs() {
        return Ok(SearchOutcome::NotFound { nodes: 0 });
    }
    let frontier = ForwardOccupancyIndex::new(table, goal);
    let constraints = (0..=rayon::current_num_threads())
        .map(|_| ReverseConstraints::new(table, &frontier))
        .collect();
    let context = Context {
        table,
        constraints,
        node_limit,
        table_limit,
        visited: ConcurrentVisited::default(),
        table_attempts: AtomicUsize::new(0),
        nodes: AtomicUsize::new(0),
        limited: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
    };
    let meeting = context.visit(goal, None, 0)?;
    let nodes = context.nodes.load(Ordering::Acquire);
    if let Some(node) = meeting {
        found(table, goal, &node, nodes)
    } else if context.limited.load(Ordering::Acquire) {
        Ok(SearchOutcome::Unknown { nodes })
    } else if context.stopped.load(Ordering::Acquire) {
        Err(io::Error::other("parallel search stopped without a result"))
    } else {
        Ok(SearchOutcome::NotFound { nodes })
    }
}
