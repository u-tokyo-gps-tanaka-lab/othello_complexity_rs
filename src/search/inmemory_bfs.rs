use std::io;
use std::sync::Mutex;

use rayon::prelude::*;

use crate::othello::{get_last_moves, Position};
use crate::record::PlyAction;
use crate::search::{
    core::{
        append_placement_predecessors, found, pass_predecessor, viable, SearchNode, SearchOutcome,
    },
    forward::ForwardTable,
    transposition::Visited,
};

struct NextLayer {
    positions: Visited,
    nodes: Vec<SearchNode>,
    meeting: Option<SearchNode>,
    limited: bool,
    expanded: usize,
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
    let root = SearchNode::root(goal);
    if table.contains(goal) {
        return found(table, goal, &root, 0);
    }
    if goal.board.popcount() <= table.discs() {
        return Ok(SearchOutcome::NotFound { nodes: 0 });
    }
    let mut current = vec![root];
    if let Some(position) = pass_predecessor(goal) {
        current.push(current[0].preceding(position, PlyAction::Pass));
    }
    if current.len() > node_limit {
        return Ok(SearchOutcome::Unknown { nodes: 0 });
    }
    let mut nodes = 0;
    while !current.is_empty() {
        let next = Mutex::new(NextLayer {
            positions: Visited::default(),
            nodes: Vec::new(),
            meeting: None,
            limited: false,
            expanded: 0,
        });
        current.par_iter().try_for_each(|node| -> io::Result<()> {
            {
                let mut next = next.lock().expect("BFS layer mutex poisoned");
                if next.limited || next.meeting.is_some() {
                    return Ok(());
                }
                next.expanded += 1;
            }
            let mut previous = Vec::new();
            append_placement_predecessors(
                node.position,
                get_last_moves(node.position.board.opponent, node.position.board.player),
                &mut previous,
            );
            for (position, action) in previous {
                if !viable(position, use_lp, use_occupancy_cache)? {
                    continue;
                }
                let child = node.preceding(position, action);
                let pass = pass_predecessor(position)
                    .map(|position| child.preceding(position, PlyAction::Pass));
                for child in std::iter::once(child).chain(pass) {
                    let mut next = next.lock().expect("BFS layer mutex poisoned");
                    if table.contains(child.position) {
                        next.meeting = Some(child);
                        return Ok(());
                    }
                    if next.limited || next.meeting.is_some() {
                        return Ok(());
                    }
                    if child.position.board.popcount() <= table.discs()
                        || next.positions.contains(&child.position)
                    {
                        continue;
                    }
                    if next.nodes.len() == node_limit - current.len() {
                        next.limited = true;
                        return Ok(());
                    }
                    next.positions.insert(child.position);
                    next.nodes.push(child);
                }
            }
            Ok(())
        })?;
        let next = next.into_inner().expect("BFS layer mutex poisoned");
        nodes += next.expanded;
        if let Some(meeting) = next.meeting {
            return found(table, goal, &meeting, nodes);
        }
        if next.limited {
            return Ok(SearchOutcome::Unknown { nodes });
        }
        current = next.nodes;
    }
    Ok(SearchOutcome::NotFound { nodes })
}
