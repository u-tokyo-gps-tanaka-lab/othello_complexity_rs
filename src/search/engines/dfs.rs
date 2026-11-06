//! Sequential depth-first reverse searches and move ordering.

use std::io;

use ordered_float::OrderedFloat;

use crate::othello::{Board, Direction, Position};
use crate::record::PlyAction;
use crate::search::{
    constraints::ReverseConstraints,
    forward::{frontier::ForwardOccupancyIndex, ForwardTable},
    moves::{append_placement_predecessors, pass_predecessor},
    outcome::{found, SearchNode, SearchOutcome},
    transposition::Btable,
};

/// in_sq : 内部のみのマスの数(8連結)
/// in_edge : 内部同士のエッジの組の数
/// sm_edge_all : 内部同士で同じ色のエッジの組の数(すべての色の合計)
/// sm_edge_min : 内部同士で同じ色のエッジの組の数で，色ごとの最小の数
fn features(b: &Board) -> (u16, u16, u16, u16) {
    let (in_sq, mut in_edge) = (0, 0);
    let mut sm_edges: [u16; 2] = [0; 2];
    let occupied = b.player | b.opponent;
    let ps: [u64; 2] = [b.player, b.opponent];
    for p in 0..2 {
        let mut p0 = ps[p];
        while p0 != 0 {
            let index = p0.trailing_zeros() as i32; // 0..=63
            p0 &= p0 - 1;
            let (x, y) = (index & 7, index >> 3);
            for dir in Direction::all().iter() {
                let (dx, dy) = dir.to_offset();
                let x1 = x + dx;
                let y1 = y + dy;
                let i1 = y1 * 8 + x1;
                if (0..8).contains(&x1) && (0..8).contains(&y1) && occupied & (1 << i1) != 0 {
                    in_edge += 1;
                    if p0 & (1 << i1) != 0 {
                        sm_edges[p] += 1;
                    }
                }
            }
        }
    }
    let sm_edges_all = (sm_edges[0] + sm_edges[1]) / 2;
    let sm_edges_min = std::cmp::min(sm_edges[0], sm_edges[1]) / 2;
    (in_sq, in_edge, sm_edges_all, sm_edges_min)
}

/// boardが到達可能かどうかを計算するヒューリスティック関数
fn h_function(b: &Board) -> f64 {
    let (in_sq, in_edge, sm_edge_sum, sm_edge_min) = features(b);
    let mut ans = 0.0;
    ans += 1.0 / (in_sq + 1) as f64;
    ans += 1.0 / (in_edge + 1) as f64;
    ans += 1.0 / (sm_edge_sum + 1) as f64;
    ans += 1.0 / (sm_edge_min + 1) as f64;
    let scount = (b.player | b.opponent).count_ones();
    ans * 2_f64.powf(scount as f64)
}

/// Cache each placement's score once, then break equal-score ties by position.
fn order_placements(previous: &mut [(Position, PlyAction)]) {
    previous
        .sort_by_cached_key(|(position, _)| (OrderedFloat(h_function(&position.board)), *position));
}

pub(super) fn order_children(children: &mut [(Position, PlyAction)]) {
    // Stable sorting preserves the established generation order on score ties.
    children.sort_by_cached_key(|(position, _)| OrderedFloat(h_function(&position.board)));
}

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

/// Explore a reverse pass first, then rank placements with the board heuristic.
/// Clear `visited` before each query; its allocated storage can be reused.
pub fn retrospective_search_move_ordering(
    goal: Position,
    table: &ForwardTable,
    visited: &mut Btable,
    node_limit: usize,
) -> io::Result<SearchOutcome> {
    search(goal, table, visited, node_limit, true)
}

fn search(
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
    let frontier = ForwardOccupancyIndex::new(table, goal);
    let constraints = ReverseConstraints::new(table, &frontier);
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
            order_placements(&mut previous);
        }
        for (position, action) in previous.into_iter().rev() {
            open.push(Work::Visit(node.preceding(position, action)));
        }
    }
    Ok(SearchOutcome::NotFound { nodes })
}
