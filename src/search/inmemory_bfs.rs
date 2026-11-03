use std::io;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

use rayon::prelude::*;

use crate::othello::{get_last_moves, Position};
use crate::record::PlyAction;
use crate::search::{
    core::{
        append_placement_predecessors, found, pass_predecessor, successors, viable, SearchNode,
        SearchOutcome,
    },
    forward::ForwardTable,
    transposition::ConcurrentVisited,
};

struct LayerShared<'a> {
    table: &'a ForwardTable,
    positions: ConcurrentVisited,
    next_count: AtomicUsize,
    stop: AtomicBool,
    meeting: Mutex<Option<Position>>,
    current_count: usize,
    node_limit: usize,
}

impl LayerShared<'_> {
    fn insert(&self, position: Position, next: &mut Vec<Position>) -> bool {
        if self.table.contains(position) {
            let mut meeting = self.meeting.lock().expect("BFS meeting mutex poisoned");
            if meeting.is_none() {
                *meeting = Some(position);
            }
            self.stop.store(true, Ordering::Release);
            return false;
        }
        if self.stop.load(Ordering::Acquire) {
            return false;
        }
        if position.board.popcount() <= self.table.discs() || !self.positions.insert(position) {
            return true;
        }
        let next_count = self.next_count.fetch_add(1, Ordering::Relaxed) + 1;
        if self.current_count.saturating_add(next_count) > self.node_limit {
            self.stop.store(true, Ordering::Release);
            return false;
        }
        next.push(position);
        true
    }
}

#[derive(Default)]
struct FoldState {
    next: Vec<Position>,
    prev_buf: Vec<(Position, PlyAction)>,
    expanded: usize,
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
/// The node limit bounds the current and next frontiers together, not the
/// number of positions expanded over the entire search.
pub fn parallel_inmemory_retrospective_bfs(
    goal: Position,
    table: &ForwardTable,
    node_limit: usize,
    use_lp: bool,
    use_occupancy_cache: bool,
) -> io::Result<SearchOutcome> {
    table.validate_goal(goal)?;
    if table.contains(goal) {
        return found(table, goal, &SearchNode::root(goal), 0);
    }
    if goal.board.popcount() <= table.discs() {
        return Ok(SearchOutcome::NotFound { nodes: 0 });
    }

    let mut first = vec![goal];
    if let Some(position) = pass_predecessor(goal) {
        first.push(position);
    }
    if first.len() > node_limit {
        return Ok(SearchOutcome::Unknown { nodes: 0 });
    }

    let standard_start = table.start() == Position::initial();
    let mut layers = vec![first];
    let mut nodes = 0;
    while !layers
        .last()
        .expect("the reverse search retains its first layer")
        .is_empty()
    {
        let current = layers
            .last()
            .expect("the reverse search retains its first layer");
        let shared = LayerShared {
            table,
            positions: ConcurrentVisited::default(),
            next_count: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            meeting: Mutex::new(None),
            current_count: current.len(),
            node_limit,
        };
        let mut next = current
            .par_iter()
            .try_fold(
                FoldState::default,
                |mut state, &position| -> io::Result<_> {
                    if shared.stop.load(Ordering::Acquire) {
                        return Ok(state);
                    }
                    state.expanded += 1;
                    state.prev_buf.clear();
                    append_placement_predecessors(
                        position,
                        get_last_moves(position.board.opponent, position.board.player),
                        &mut state.prev_buf,
                    );
                    for (previous, _) in state.prev_buf.iter().copied() {
                        if standard_start && !viable(previous, use_lp, use_occupancy_cache)? {
                            continue;
                        }
                        let pass = pass_predecessor(previous);
                        if !shared.insert(previous, &mut state.next) {
                            return Ok(state);
                        }
                        if let Some(pass) = pass {
                            if !shared.insert(pass, &mut state.next) {
                                return Ok(state);
                            }
                        }
                    }
                    Ok(state)
                },
            )
            .try_reduce(FoldState::default, |mut left, mut right| {
                left.next.append(&mut right.next);
                left.expanded += right.expanded;
                Ok(left)
            })?;
        nodes += next.expanded;

        let stopped = shared.stop.load(Ordering::Acquire);
        let meeting = shared
            .meeting
            .into_inner()
            .expect("BFS meeting mutex poisoned");
        if let Some(meeting) = meeting {
            layers.push(std::mem::take(&mut next.next));
            let node = meeting_node(goal, meeting, &mut layers)?;
            return found(table, goal, &node, nodes);
        }
        if stopped {
            return Ok(SearchOutcome::Unknown { nodes });
        }
        layers.push(next.next);
    }
    Ok(SearchOutcome::NotFound { nodes })
}
