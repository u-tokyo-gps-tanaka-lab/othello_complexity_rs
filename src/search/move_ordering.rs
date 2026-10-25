use std::io;

use ordered_float::OrderedFloat;

use crate::othello::{east, ne, north, nw, Board, Position};
use crate::record::PlyAction;
use crate::search::{core::SearchOutcome, forward::ForwardTable, transposition::Btable};

/// in_sq : 内部のみのマスの数(8連結)
/// in_edge : 内部同士のエッジの組の数
/// sm_edge_all : 内部同士で同じ色のエッジの組の数(すべての色の合計)
/// sm_edge_min : 内部同士で同じ色のエッジの組の数で，色ごとの最小の数
fn features(b: &Board) -> (u16, u16, u16, u16) {
    // Four forward directions count each undirected 8-neighbour edge once.
    // The original loop counted occupied edges twice, but same-colour edges
    // only once because it removed lower-index stones while scanning them.
    fn adjacent_pairs(mask: u64) -> u16 {
        ((east(mask) & mask).count_ones()
            + (north(mask) & mask).count_ones()
            + (ne(mask) & mask).count_ones()
            + (nw(mask) & mask).count_ones()) as u16
    }
    let in_sq = 0;
    let in_edge = 2 * adjacent_pairs(b.player | b.opponent);
    let sm_edges = [adjacent_pairs(b.player), adjacent_pairs(b.opponent)];
    // Preserve the established score, including its integer divisions, so all
    // search priorities and tie breaks remain unchanged.
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
    // The number of stones is in 0..=64. Its power of two is represented
    // exactly by this exponent, avoiding a general floating-point pow call.
    ans * f64::from_bits((u64::from(scount) + 1023) << 52)
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

#[cfg(test)]
mod tests {
    use rand::{Rng, SeedableRng};

    use super::{features, h_function};
    use crate::othello::{Board, Direction};

    // Retain the square-by-square calculation as an independent oracle for
    // the bitboard replacement, including its historical rounding behaviour.
    fn reference_features(b: &Board) -> (u16, u16, u16, u16) {
        let mut edges = 0;
        let mut same = [0_u16; 2];
        let occupied = b.player | b.opponent;
        for (color, mask) in [b.player, b.opponent].into_iter().enumerate() {
            let mut remaining = mask;
            while remaining != 0 {
                let index = remaining.trailing_zeros() as i32;
                remaining &= remaining - 1;
                let (x, y) = (index % 8, index / 8);
                for direction in Direction::all() {
                    let (dx, dy) = direction.to_offset();
                    let (x1, y1) = (x + dx, y + dy);
                    if (0..8).contains(&x1) && (0..8).contains(&y1) {
                        let bit = 1_u64 << (8 * y1 + x1);
                        if occupied & bit != 0 {
                            edges += 1;
                            if remaining & bit != 0 {
                                same[color] += 1;
                            }
                        }
                    }
                }
            }
        }
        (0, edges, (same[0] + same[1]) / 2, same[0].min(same[1]) / 2)
    }

    fn assert_identical(board: Board) {
        let expected = reference_features(&board);
        assert_eq!(features(&board), expected, "board={board:?}");
        let (a, b, c, d) = expected;
        let score = (1.0 / f64::from(a + 1)
            + 1.0 / f64::from(b + 1)
            + 1.0 / f64::from(c + 1)
            + 1.0 / f64::from(d + 1))
            * 2_f64.powf(f64::from(board.popcount()));
        assert_eq!(h_function(&board).to_bits(), score.to_bits());
    }

    #[test]
    fn bitboard_heuristic_preserves_scores_and_edge_boundaries() {
        for first in 0..64 {
            for second in 0..64 {
                let first = 1_u64 << first;
                let second = (1_u64 << second) & !first;
                assert_identical(Board::new(first | second, 0));
                assert_identical(Board::new(first, second));
            }
        }
        assert_identical(Board::new(0, 0));
        assert_identical(Board::new(u64::MAX, 0));
        assert_identical(Board::new(0x5555_5555_5555_5555, 0xaaaa_aaaa_aaaa_aaaa));
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xb17b_0a2d);
        for _ in 0..4096 {
            let occupied = rng.random::<u64>();
            let player = rng.random::<u64>() & occupied;
            assert_identical(Board::new(player, occupied & !player));
        }
    }
}
