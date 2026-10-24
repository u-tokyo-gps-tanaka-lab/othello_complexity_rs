use std::io;

use crate::othello::Position;
use crate::record::PlyAction;
use crate::search::{
    constraints::ReverseConstraints,
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
        Placements(SearchNode, u64),
    }

    table.validate_goal(goal)?;
    if table.contains(goal) {
        return found(table, goal, &SearchNode::root(goal), 0);
    }
    if goal.board.popcount() <= table.discs() {
        return Ok(SearchOutcome::NotFound { nodes: 0 });
    }
    let constraints = ReverseConstraints::new(table, goal);
    let mut open = vec![Work::Visit(SearchNode::root(goal))];
    let mut nodes = 0;
    while let Some(work) = open.pop() {
        let (node, candidates) = match work {
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
                let Some(candidates) = constraints.candidates(node.position) else {
                    continue;
                };
                if let Some(previous) = pass_predecessor(node.position) {
                    let pass = node.preceding(previous, PlyAction::Pass);
                    // Resume placements only if the entire pass branch fails.
                    // A found path or node-limit stop returns before this work.
                    open.push(Work::Placements(node, candidates));
                    open.push(Work::Visit(pass));
                    continue;
                }
                (node, candidates)
            }
            // The node was already counted and checked before its pass branch.
            Work::Placements(node, candidates) => (node, candidates),
        };
        let mut previous = Vec::new();
        append_placement_predecessors(node.position, candidates, &mut previous);
        if ordered {
            crate::search::move_ordering::order_placements(&mut previous);
        }
        for (position, action) in previous.into_iter().rev() {
            open.push(Work::Visit(node.preceding(position, action)));
        }
    }
    Ok(SearchOutcome::NotFound { nodes })
}
