use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use ordered_float::OrderedFloat;

use crate::othello::Position;
use crate::filters::{occupancy::check_occupancy, seg3::check_seg3_more};
use crate::record::PlyAction;
use crate::search::{
    core::{found, predecessors, SearchNode, SearchOutcome},
    forward::ForwardTable,
    move_ordering::h_function,
    transposition::Visited,
};

struct Progress {
    visited: Visited,
    table_attempts: usize,
    nodes: usize,
    limited: bool,
}

struct Context<'a> {
    table: &'a ForwardTable,
    node_limit: usize,
    table_limit: usize,
    progress: Mutex<Progress>,
    stopped: AtomicBool,
}

fn order_children(children: &mut [(Position, PlyAction)]) {
    // Stable sorting preserves the established generation order on score ties.
    children.sort_by_cached_key(|(position, _)| OrderedFloat(h_function(&position.board)));
}

impl Context<'_> {
    fn visit(&self, node: SearchNode, depth: usize) -> io::Result<Option<SearchNode>> {
        if self.stopped.load(Ordering::Acquire) {
            return Ok(None);
        }
        // A task that has already started can still supply an actual witness
        // while another task reaches the resource limit.
        if self.table.contains(node.position) {
            self.stopped.store(true, Ordering::Release);
            return Ok(Some(node));
        }
        if node.position.board.popcount() <= self.table.discs() {
            return Ok(None);
        }
        {
            let mut progress = self.progress.lock().expect("search mutex poisoned");
            if self.stopped.load(Ordering::Acquire) {
                return Ok(None);
            }
            progress.table_attempts += 1;
            if progress.table_attempts < self.table_limit {
                if !progress.visited.insert(node.position) {
                    return Ok(None);
                }
            } else if progress.visited.contains(&node.position) {
                return Ok(None);
            }
            progress.nodes += 1;
            if progress.nodes > self.node_limit {
                progress.limited = true;
                self.stopped.store(true, Ordering::Release);
                return Ok(None);
            }
        }
        let board = node.position.board;
        if !check_occupancy(board.player | board.opponent)
            || !check_seg3_more(board.player, board.opponent, false)
        {
            return Ok(None);
        }
        let mut previous = predecessors(node.position);
        order_children(&mut previous);
        if depth < 12 && previous.len() >= 4 {
            let mut children = previous.into_iter();
            let (position, action) = children.next().expect("split has at least four children");
            // Finish the best-ranked branch before issuing the remaining FIFO
            // tasks, as in the original dynamic parallel DFS.
            let first = self.visit(node.preceding(position, action), depth + 1)?;
            if first.is_some() || self.stopped.load(Ordering::Acquire) {
                return Ok(first);
            }
            let joined = Mutex::new(Ok(None));
            rayon::scope_fifo(|scope| {
                for (position, action) in children {
                    let child = node.preceding(position, action);
                    let joined = &joined;
                    scope.spawn_fifo(move |_| {
                        let outcome = self.visit(child, depth + 1);
                        let mut joined = joined.lock().expect("search mutex poisoned");
                        match outcome {
                            Err(error) => {
                                self.stopped.store(true, Ordering::Release);
                                *joined = Err(error);
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
                if let Some(meeting) = self.visit(node.preceding(position, action), depth + 1)? {
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
pub fn retrospective_search_parallel(
    goal: Position,
    table: &ForwardTable,
    node_limit: usize,
    table_limit: usize,
    threads: usize,
) -> io::Result<SearchOutcome> {
    table.validate_goal(goal)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(io::Error::other)?;
    let context = Context {
        table,
        node_limit,
        table_limit,
        progress: Mutex::new(Progress {
            visited: Visited::default(),
            table_attempts: 0,
            nodes: 0,
            limited: false,
        }),
        stopped: AtomicBool::new(false),
    };
    let meeting = pool.install(|| context.visit(SearchNode::root(goal), 0))?;
    let progress = context
        .progress
        .into_inner()
        .expect("search mutex poisoned");
    if let Some(node) = meeting {
        found(table, goal, &node, progress.nodes)
    } else if progress.limited {
        Ok(SearchOutcome::Unknown {
            nodes: progress.nodes,
        })
    } else {
        Ok(SearchOutcome::NotFound {
            nodes: progress.nodes,
        })
    }
}
