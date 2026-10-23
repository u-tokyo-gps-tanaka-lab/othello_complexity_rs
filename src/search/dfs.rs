use std::io;

use crate::filters::{occupancy::check_occupancy, seg3::check_seg3_more};
use crate::othello::Position;
use crate::record::PlyAction;
use crate::search::{
    core::{append_placement_predecessors, found, pass_predecessor, SearchNode, SearchOutcome},
    forward::ForwardTable,
    transposition::Btable,
};

/// Depth-first search over exact one-action predecessors.
/// Clear `visited` before each query; its allocated storage can be reused.
pub fn retrospective_search(
    goal: Position,
    table: &ForwardTable,
    visited: &mut Btable,
    node_limit: usize,
) -> io::Result<SearchOutcome> {
    search(goal, table, visited, node_limit, false)
}

pub(crate) fn search(
    goal: Position,
    table: &ForwardTable,
    visited: &mut Btable,
    node_limit: usize,
    ordered: bool,
) -> io::Result<SearchOutcome> {
    enum Work {
        Visit(SearchNode),
        Placements(SearchNode),
    }

    table.validate_goal(goal)?;
    let mut open = vec![Work::Visit(SearchNode::root(goal))];
    let mut nodes = 0;
    while let Some(work) = open.pop() {
        let node = match work {
            Work::Visit(node) => {
                if table.contains(node.position) {
                    return found(table, goal, &node, nodes);
                }
                if node.position.board.popcount() <= table.discs() {
                    continue;
                }
                if !visited.insert(node.position) {
                    continue;
                }
                nodes += 1;
                if nodes > node_limit {
                    return Ok(SearchOutcome::Unknown { nodes });
                }
                let board = node.position.board;
                if !check_occupancy(board.player | board.opponent)
                    || !check_seg3_more(board.player, board.opponent, false)
                {
                    continue;
                }
                if let Some(previous) = pass_predecessor(node.position) {
                    let pass = node.preceding(previous, PlyAction::Pass);
                    // Resume placements only if the entire pass branch fails.
                    // A found path or node-limit stop returns before this work.
                    open.push(Work::Placements(node));
                    open.push(Work::Visit(pass));
                    continue;
                }
                node
            }
            // The node was already counted and checked before its pass branch.
            Work::Placements(node) => node,
        };
        let mut previous = Vec::new();
        append_placement_predecessors(node.position, u64::MAX, &mut previous);
        if ordered {
            crate::search::move_ordering::order_placements(&mut previous);
        }
        for (position, action) in previous.into_iter().rev() {
            open.push(Work::Visit(node.preceding(position, action)));
        }
    }
    Ok(SearchOutcome::NotFound { nodes })
}
