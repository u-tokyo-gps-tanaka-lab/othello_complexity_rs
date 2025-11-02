use std::collections::HashSet;

use crate::{
    othello::{get_moves, Board, Direction},
    prunings::{occupancy::check_occupancy, seg3::check_seg3_more},
    search::core::{retrospective_flip, Btable, SearchResult},
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
                if 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && occupied & (1 << i1) != 0 {
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

/// retrospective_searchでmove orderingを実行するバージョン
/// - `from_pass`: 直前にパスで1手分遡ったか否か
/// - `discs`: 順方向探索の深さ（石数）
/// - `leafnode`: 順方向探索で得たuniqueなleafnodeの集合（しきい値以上で合法手があるもの）
/// - `retrospective_searched`: 既訪問ユニーク局面
/// - `retroflips`: ディスク数ごとに使い回す作業バッファ（長さ 10_000 の配列を入れておく）
///   インデックスは `num_disc as usize` を想定。必要に応じて拡張する。
pub fn retrospective_search_move_ordering(
    board: &Board,
    from_pass: bool,
    discs: i32,
    leafnode: &HashSet<[u64; 2]>,
    retrospective_searched: &mut Btable,
    retroflips: &mut Vec<[u64; 10_000]>,
    node_count: &mut usize,
    node_limit: usize,
) -> SearchResult {
    let uni = board.unique();
    let num_disc = board.popcount() as usize;

    // 順方向探索の leafnode に含まれているか確認
    if (num_disc as i32) <= discs {
        return if leafnode.contains(&uni) {
            println!("info: found unique board in leafnodes:");
            println!("unique player = {}", uni[0]);
            println!("unique opponent = {}", uni[1]);
            println!("board player = {}", board.player);
            println!("board opponent = {}", board.opponent);
            SearchResult::Found
        } else {
            SearchResult::NotFound
        };
    }

    // 再訪防止
    if !retrospective_searched.insert(uni) {
        return SearchResult::NotFound;
    }
    *node_count += 1;
    if *node_count > node_limit {
        return SearchResult::Unknown;
    }
    //if retrospective_searched.len() > node_limit {
    //    return SearchResult::Unknown;
    //}
    //if retrospective_searched.len() > 0x20000000 {
    //    eprintln!(
    //        "Memory overflow: visited={}, node_limit={}, discs={}, from_pass={}",
    //        retrospective_searched.len(),
    //        node_limit,
    //        num_disc,
    //        from_pass
    //    );
    //    return SearchResult::Unknown;
    //}

    let occupied = board.player | board.opponent;
    //if !is_connected(occupied) {
    //    return SearchResult::NotFound;
    //}
    //if !check_seg3(occupied) {
    //    return SearchResult::NotFound;
    //}
    if !check_occupancy(occupied) {
        return SearchResult::NotFound;
    }
    if !check_seg3_more(board.player, board.opponent) {
        return SearchResult::NotFound;
    }
    // let line = board.to_string();
    // if !is_sat_ok(0, &line).unwrap() {
    //     return false;
    // }

    // パスの処理
    // from_pass==false かつ 相手に合法手が無いならば、1手前に相手がパスしたと仮定
    if !from_pass {
        if get_moves(board.opponent, board.player) == 0 {
            let prev = Board {
                player: board.opponent,
                opponent: board.player,
            };
            match retrospective_search_move_ordering(
                &prev,
                true,
                discs,
                leafnode,
                retrospective_searched,
                retroflips,
                node_count,
                node_limit,
            ) {
                SearchResult::Found => {
                    println!("pass found");
                    return SearchResult::Found;
                }
                SearchResult::Unknown => {
                    println!("pass found");
                    return SearchResult::Unknown;
                }
                SearchResult::NotFound => {}
            }
        }
    }

    // 相手石（中央4マス以外）を候補として走査
    let mut b = board.opponent & !0x0000_0018_1800_0000u64;
    if b == 0 {
        return SearchResult::NotFound;
    }

    // retroflips[num_disc] を使うので、足りなければ拡張
    if retroflips.len() <= num_disc {
        retroflips.resize(num_disc + 1, [0u64; 10_000]);
    }

    // （デバッグ用カウンタ：C++ と同様に保持するが使っていない）
    let mut _searched: i32 = 0;

    let mut next_w_score: Vec<(f64, Board)> = vec![];
    while b != 0 {
        let index = b.trailing_zeros(); // 0..=63
        b &= b - 1;

        // “直前に相手が index に置いた” と想定したときの可能 flip 集合を列挙
        let num = retrospective_flip(
            index,
            board.player,
            board.opponent,
            &mut retroflips[num_disc],
        );
        if num > 0 {
            // result[0] は 0（便宜上）なので、-1 した数だけ “実 flips” を見た回数として数える
            _searched += (num - 1) as i32;
        }

        for i in 1..num {
            let flipped = retroflips[num_disc][i];
            debug_assert!(flipped != 0);

            let prev = Board {
                // 直前に相手が index に置き、flipped が返ったと仮定した局面の 1 手前
                player: board.opponent ^ (flipped | (1u64 << index)),
                opponent: board.player ^ flipped,
            };
            next_w_score.push((h_function(&prev), prev));
            // next_w_score.push((0.0, prev));
        }
    }
    next_w_score.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    for i in 0..next_w_score.len() {
        let (_, prev) = next_w_score[i];
        match retrospective_search_move_ordering(
            &prev,
            false,
            discs,
            leafnode,
            retrospective_searched,
            retroflips,
            node_count,
            node_limit,
        ) {
            SearchResult::Found => {
                // println!("{}", index);
                return SearchResult::Found;
            }
            SearchResult::Unknown => {
                // println!("{}", index);
                return SearchResult::Unknown;
            }
            SearchResult::NotFound => {}
        }
    }
    SearchResult::NotFound
}
