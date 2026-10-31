//! Bounded breadth-first reverse search used before an exact SAT call.
//!
//! The search returns `Found` when it meets the forward table, `NotFound` when
//! it exhausts the reverse search, and `Frontier` when it exceeds the state
//! budget.

use std::io;

use crate::othello::Position;
use crate::record::PlyAction;
use crate::search::{
    constraints::ReverseConstraints,
    core::{found, pass_predecessor, PlacementPredecessors, SearchNode, SearchOutcome},
    forward::ForwardTable,
    transposition::Visited,
};

#[derive(Debug)]
pub enum BfsOutcome {
    Resolved(SearchOutcome),
    Frontier { nodes: usize },
}

struct Node {
    position: Position,
    candidates: u64,
    parent: usize,
    action: PlyAction,
}

/// Search with at most `state_budget` registered positions.
pub fn retrospective_bfs(
    goal: Position,
    table: &ForwardTable,
    state_budget: usize,
) -> io::Result<BfsOutcome> {
    table.validate_goal(goal)?;
    if table.contains(goal) {
        return Ok(BfsOutcome::Resolved(found(
            table,
            goal,
            &SearchNode::root(goal),
            0,
        )?));
    }
    if goal.board.popcount() <= table.discs() {
        return Ok(BfsOutcome::Resolved(SearchOutcome::NotFound { nodes: 0 }));
    }
    let constraints = ReverseConstraints::new(table, goal);
    let mut nodes: Vec<Node> = Vec::new();
    let mut seen = Visited::default();
    let Some(candidates) = constraints.candidates(goal) else {
        return Ok(BfsOutcome::Resolved(SearchOutcome::NotFound { nodes: 0 }));
    };
    nodes.push(Node {
        position: goal,
        candidates,
        parent: usize::MAX,
        action: PlyAction::Pass,
    });
    seen.insert(goal);
    if let Some(previous) = pass_predecessor(goal) {
        // The pass predecessor has the goal's stone count, so it lies above
        // the table like the goal itself and cannot meet it here.
        register(
            &mut nodes,
            &mut seen,
            table,
            &constraints,
            previous,
            0,
            PlyAction::Pass,
        )?;
    }
    let mut level_start = 0;
    loop {
        let level_end = nodes.len();
        for index in level_start..level_end {
            let (position, candidates) = (nodes[index].position, nodes[index].candidates);
            for (previous, action) in PlacementPredecessors::new(position, candidates) {
                let Some(child) = register(
                    &mut nodes,
                    &mut seen,
                    table,
                    &constraints,
                    previous,
                    index,
                    action,
                )?
                else {
                    continue;
                };
                let Some(child) = child else {
                    return Ok(BfsOutcome::Resolved(found(
                        table,
                        goal,
                        &reverse_node(&nodes, previous, index, action),
                        nodes.len(),
                    )?));
                };
                if let Some(passed) = pass_predecessor(previous) {
                    if let Some(None) = register(
                        &mut nodes,
                        &mut seen,
                        table,
                        &constraints,
                        passed,
                        child,
                        PlyAction::Pass,
                    )? {
                        return Ok(BfsOutcome::Resolved(found(
                            table,
                            goal,
                            &reverse_node(&nodes, passed, child, PlyAction::Pass),
                            nodes.len(),
                        )?));
                    }
                }
            }
            if nodes.len() > state_budget {
                return Ok(BfsOutcome::Frontier { nodes: nodes.len() });
            }
        }
        if nodes.len() == level_end {
            return Ok(BfsOutcome::Resolved(SearchOutcome::NotFound {
                nodes: nodes.len(),
            }));
        }
        level_start = level_end;
    }
}

/// Register a predecessor. `Ok(None)` is a dropped or duplicate position,
/// `Ok(Some(None))` a meeting with the forward table, and `Ok(Some(Some(i)))`
/// the index of a newly registered node.
fn register(
    nodes: &mut Vec<Node>,
    seen: &mut Visited,
    table: &ForwardTable,
    constraints: &ReverseConstraints,
    position: Position,
    parent: usize,
    action: PlyAction,
) -> io::Result<Option<Option<usize>>> {
    if table.contains(position) {
        return Ok(Some(None));
    }
    if position.board.popcount() <= table.discs() || seen.contains(&position) {
        return Ok(None);
    }
    let Some(candidates) = constraints.candidates(position) else {
        // Remember the rejection so the same position is not re-checked.
        seen.insert(position);
        return Ok(None);
    };
    seen.insert(position);
    nodes.push(Node {
        position,
        candidates,
        parent,
        action,
    });
    Ok(Some(Some(nodes.len() - 1)))
}

fn reverse_node(
    nodes: &[Node],
    position: Position,
    parent: usize,
    action: PlyAction,
) -> SearchNode {
    let mut reverse_actions = vec![action];
    let mut index = parent;
    while index != usize::MAX {
        let node = &nodes[index];
        if node.parent == usize::MAX {
            break;
        }
        reverse_actions.push(node.action);
        index = node.parent;
    }
    // Actions were collected from the meeting position towards the goal, but
    // SearchNode stores them from the goal towards the node.
    reverse_actions.reverse();
    SearchNode {
        position,
        reverse_actions,
    }
}
