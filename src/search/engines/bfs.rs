//! Parallel in-memory breadth-first reverse search.

use std::io;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

use rayon::prelude::*;

use crate::othello::Position;
use crate::record::PlyAction;
use crate::search::{
    constraints::{passes_lp, ReverseConstraints},
    forward::{frontier::ForwardOccupancyIndex, ForwardTable},
    moves::{pass_predecessor, successors, PlacementPredecessors},
    outcome::{found, SearchNode, SearchOutcome},
    transposition::ConcurrentVisited,
};

struct Node {
    position: Position,
    candidates: u64,
}

fn positions(layer: Vec<Node>) -> Vec<Position> {
    layer.into_iter().map(|node| node.position).collect()
}

struct LayerShared<'a, 'b> {
    table: &'a ForwardTable,
    constraints: &'a [ReverseConstraints<'b>],
    visited: ConcurrentVisited,
    nodes: &'a AtomicUsize,
    max_nodes: usize,
    use_lp: bool,
    standard_start: bool,
    stop: AtomicBool,
    meeting: Mutex<Option<Position>>,
}

impl LayerShared<'_, '_> {
    fn constraints(&self) -> &ReverseConstraints<'_> {
        let index = rayon::current_thread_index().unwrap_or(self.constraints.len() - 1);
        &self.constraints[index]
    }

    fn register(&self, position: Position, layer: &mut Vec<Node>) -> io::Result<bool> {
        if self.table.contains(position) {
            let mut meeting = self.meeting.lock().expect("BFS meeting mutex poisoned");
            if meeting.is_none() {
                *meeting = Some(position);
            }
            self.stop.store(true, Ordering::Release);
            return Ok(false);
        }
        if self.stop.load(Ordering::Acquire) {
            return Ok(false);
        }
        if position.board.popcount() <= self.table.discs() {
            return Ok(false);
        }
        if !self.visited.insert(position) {
            return Ok(false);
        }
        let Some(candidates) = self.constraints().candidates(position) else {
            return Ok(false);
        };
        if self.use_lp && self.standard_start && !passes_lp(position)? {
            return Ok(false);
        }
        let nodes = self.nodes.fetch_add(1, Ordering::Relaxed) + 1;
        if nodes > self.max_nodes {
            self.stop.store(true, Ordering::Release);
            return Ok(false);
        }
        layer.push(Node {
            position,
            candidates,
        });
        Ok(true)
    }
}

fn meeting_node(
    goal: Position,
    meeting: Position,
    layers: &mut [Vec<Position>],
) -> io::Result<SearchNode> {
    for layer in layers.iter_mut() {
        layer.par_sort_unstable();
    }

    let mut position = meeting;
    let mut layer_index = layers
        .len()
        .checked_sub(1)
        .expect("a meeting has at least one reverse-search layer");
    let mut reverse_actions = Vec::new();
    while position != goal {
        let next = successors(position).into_iter().find_map(|(next, action)| {
            let next_layer = match action {
                PlyAction::Play(_) => layer_index.checked_sub(1)?,
                PlyAction::Pass => layer_index,
            };
            layers[next_layer]
                .binary_search(&next)
                .is_ok()
                .then_some((next, action, next_layer))
        });
        let Some((next, action, next_layer)) = next else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "reverse-search layers contain no path from the meeting position to the goal",
            ));
        };
        position = next;
        layer_index = next_layer;
        reverse_actions.push(action);
    }
    reverse_actions.reverse();
    Ok(SearchNode::from_reverse_actions(meeting, reverse_actions))
}

/// Breadth-first search by stone count. Legal passes stay in the same layer.
/// The node limit bounds all distinct positions registered by the search.
pub fn retrospective_bfs(
    goal: Position,
    table: &ForwardTable,
    max_nodes: usize,
    use_lp: bool,
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
        .collect::<Vec<_>>();
    let nodes = AtomicUsize::new(0);
    let standard_start = table.start() == Position::initial();
    let make_shared = || LayerShared {
        table,
        constraints: &constraints,
        visited: ConcurrentVisited::default(),
        nodes: &nodes,
        max_nodes,
        use_lp,
        standard_start,
        stop: AtomicBool::new(false),
        meeting: Mutex::new(None),
    };
    let initial = make_shared();
    let mut first = Vec::new();
    if initial.register(goal, &mut first)? {
        if let Some(position) = pass_predecessor(goal) {
            initial.register(position, &mut first)?;
        }
    }

    if initial.stop.load(Ordering::Acquire) {
        return Ok(SearchOutcome::Unknown {
            nodes: nodes.load(Ordering::Relaxed),
        });
    }
    if first.is_empty() {
        return Ok(SearchOutcome::NotFound {
            nodes: nodes.load(Ordering::Relaxed),
        });
    }

    let mut current = first;
    let mut layers = Vec::new();
    loop {
        let shared = make_shared();
        let next = current
            .par_iter()
            .try_fold(Vec::new, |mut next, node| -> io::Result<_> {
                if shared.stop.load(Ordering::Acquire) {
                    return Ok(next);
                }
                for (previous, _) in PlacementPredecessors::new(node.position, node.candidates) {
                    if shared.register(previous, &mut next)? {
                        if let Some(position) = pass_predecessor(previous) {
                            shared.register(position, &mut next)?;
                        }
                    }
                    if shared.stop.load(Ordering::Acquire) {
                        return Ok(next);
                    }
                }
                Ok(next)
            })
            .try_reduce(Vec::new, |mut left, mut right| {
                left.append(&mut right);
                Ok(left)
            })?;

        layers.push(positions(current));
        let meeting = shared
            .meeting
            .into_inner()
            .expect("BFS meeting mutex poisoned");
        if let Some(meeting) = meeting {
            layers.push(positions(next));
            let node = meeting_node(goal, meeting, &mut layers)?;
            return found(table, goal, &node, nodes.load(Ordering::Relaxed));
        }
        if shared.stop.load(Ordering::Acquire) {
            return Ok(SearchOutcome::Unknown {
                nodes: nodes.load(Ordering::Relaxed),
            });
        }
        if next.is_empty() {
            return Ok(SearchOutcome::NotFound {
                nodes: nodes.load(Ordering::Relaxed),
            });
        }
        current = next;
    }
}
