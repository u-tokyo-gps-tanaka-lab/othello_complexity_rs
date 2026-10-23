use std::io;

use ordered_float::OrderedFloat;

use crate::othello::{Board, Direction, Position};
use crate::record::PlyAction;
use crate::search::{core::SearchOutcome, forward::ForwardTable, transposition::Btable};

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
pub fn h_function(b: &Board) -> f64 {
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
pub(super) fn order_placements(previous: &mut [(Position, PlyAction)]) {
    previous
        .sort_by_cached_key(|(position, _)| (OrderedFloat(h_function(&position.board)), *position));
}

/// Explore a reverse pass first, then rank placements with the board heuristic.
/// Clear `visited` before each query; its allocated storage can be reused.
pub fn retrospective_search_move_ordering(
    goal: Position,
    table: &ForwardTable,
    visited: &mut Btable,
    node_limit: usize,
) -> io::Result<SearchOutcome> {
    crate::search::dfs::search(goal, table, visited, node_limit, true)
}
