use crate::lib::othello::{flip, get_moves, Board, DXYS};

use std::cmp::min;
use std::collections::HashSet;

// translated with ChatGPT 4o
/**
 * retrospective-dfs-reversi
 *
 * https://github.com/eukaryo/retrospective-dfs-reversi
 *
 * @date 2020
 * @author Hiroki Takizawa
 */

/// 盤面 `b` が 8 近傍で連結しているかを判定する関数。
/// 中央4マス(初期配置)が必ず含まれる前提です。
fn is_connected(b: u64) -> bool {
    let mut mark: u64 = 0x0000_0018_1800_0000u64;
    let mut old_mark: u64 = 0;

    // 中央 4 マスが存在しているか確認
    assert!((b & mark) == mark);

    // マークが更新されなくなるまでループ
    while mark != old_mark {
        old_mark = mark;
        let mut new_mark = mark;

        new_mark |= b & ((mark & 0xFEFE_FEFE_FEFE_FEFEu64) >> 1);
        new_mark |= b & ((mark & 0x7F7F_7F7F_7F7F_7F7Fu64) << 1);
        new_mark |= b & ((mark & 0xFFFF_FFFF_FFFF_FF00u64) >> 8);
        new_mark |= b & ((mark & 0x00FF_FFFF_FFFF_FFFFu64) << 8);
        new_mark |= b & ((mark & 0x7F7F_7F7F_7F7F_7F00u64) >> 7);
        new_mark |= b & ((mark & 0x00FE_FEFE_FEFE_FEFEu64) << 7);
        new_mark |= b & ((mark & 0xFEFE_FEFE_FEFE_FE00u64) >> 9);
        new_mark |= b & ((mark & 0x007F_7F7F_7F7F_7F7Fu64) << 9);

        mark = new_mark;
    }

    // 全ての石がマークされていれば連結とみなす
    mark == b
}

fn no_cycle(g: Vec<Vec<usize>>) -> bool {
    let mut icount = vec![0; 64];
    for i in 0..64 {
        for &j in &g[i] {
            icount[j] += 1;
        }
    }
    let mut q = vec![];
    for i in 0..64 {
        if g[i].len() > 0 && icount[i] == 0 {
            q.push(i);
        }
    }
    while q.len() > 0 {
        let i = q.pop().unwrap();
        for &j in &g[i as usize] {
            icount[j] -= 1;
            if icount[j] == 0 {
                q.push(j);
            }
        }
    }
    for i in 0..64 {
        if icount[i] > 0 {
            return false;
        }
    }
    true
}

pub fn check_seg3(b: u64) -> bool {
    let mut g: Vec<Vec<usize>> = vec![vec![]; 64];
    for y in 0..8 {
        for x in 0..8 {
            let i = y * 8 + x;
            if b & (1 << i) == 0 {
                continue;
            }
            if 3 <= x && x <= 4 && 3 <= y && y <= 4 {
                continue;
            }
            let mut oks: Vec<Vec<usize>> = vec![];
            for (dx, dy) in DXYS.iter() {
                let mut l = 1;
                let mut x1 = x + dx;
                let mut y1 = y + dy;
                let mut i1 = y1 * 8 + x1;
                while 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && b & (1 << i1) != 0 {
                    l += 1;
                    x1 += dx;
                    y1 += dy;
                    i1 = y1 * 8 + x1;
                }
                if l >= 3 {
                    let di = dy * 8 + dx;
                    oks.push(vec![(i + di) as usize, (i + di * 2) as usize]);
                }
            }
            if oks.len() == 0 {
                return false;
            }
            if oks.len() == 1 {
                g[i as usize].push(oks[0][0]);
                g[i as usize].push(oks[0][1]);
            }
        }
    }
    return no_cycle(g);
}

pub fn search(
    board: &Board,
    searched: &mut HashSet<[u64; 2]>,
    leafnode: &mut HashSet<[u64; 2]>,
    discs: i32,
) {
    let uni = board.unique();

    if board.popcount() >= discs as u32 {
        if get_moves(board.player, board.opponent) != 0 {
            leafnode.insert(uni);
            return;
        } else if get_moves(board.opponent, board.player) != 0 {
            let next = Board {
                player: board.opponent,
                opponent: board.player,
            };
            search(&next, searched, leafnode, discs);
        }
        return;
    }

    if !searched.insert(uni) {
        return;
    }

    let mut moves = get_moves(board.player, board.opponent);
    if moves == 0 {
        if get_moves(board.opponent, board.player) != 0 {
            let next = Board {
                player: board.opponent,
                opponent: board.player,
            };
            search(&next, searched, leafnode, discs);
        }
        return;
    }
    // println!("{}", board.show());
    // println!("moves={}", mask_to_moves(moves));
    while moves != 0 {
        let idx = moves.trailing_zeros();
        moves &= moves - 1;

        let flipped = flip(idx as usize, board.player, board.opponent);
        if flipped == 0 {
            continue;
        }
        let next = Board {
            player: board.opponent ^ flipped,
            opponent: board.player ^ (flipped | (1u64 << idx)),
        };
        search(&next, searched, leafnode, discs);
    }
}

/// Tri-state result for limited search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResult {
    Found,
    NotFound,
    Unknown, // node limit exceeded or resource constraint
}

/// Retrospective search with a node (unique-state) limit.
/// Behaves like `retrospective_search`, but returns `Unknown` if the number of
/// unique states visited exceeds `node_limit`.
pub fn retrospective_search_limited(
    board: &Board,
    from_pass: bool,
    discs: i32,
    leafnode: &HashSet<[u64; 2]>,
    retrospective_searched: &mut HashSet<[u64; 2]>,
    retroflips: &mut Vec<[u64; 10_000]>,
    node_limit: usize,
) -> SearchResult {
    let uni = board.unique();
    let num_disc = board.popcount() as usize;

    // Threshold: if at or below discs, only check membership in leafnode.
    if (num_disc as i32) <= discs {
        return if leafnode.contains(&uni) {
            SearchResult::Found
        } else {
            SearchResult::NotFound
        };
    }

    // Revisit check
    if !retrospective_searched.insert(uni) {
        return SearchResult::NotFound;
    }
    // Node limit check
    if retrospective_searched.len() > node_limit {
        return SearchResult::Unknown;
    }
    // Very large hard cap (previous behavior printed and returned true). Treat as Unknown.
    if retrospective_searched.len() > 0x20000000 {
        return SearchResult::Unknown;
    }

    // Prune by connectivity and segment-3 constraint
    let occupied = board.player | board.opponent;
    if !is_connected(occupied) {
        return SearchResult::NotFound;
    }
    if !check_seg3(occupied) {
        return SearchResult::NotFound;
    }

    // Pass backward (only once in a row)
    if !from_pass {
        if get_moves(board.opponent, board.player) == 0 {
            let prev = Board {
                player: board.opponent,
                opponent: board.player,
            };
            match retrospective_search_limited(
                &prev,
                true,
                discs,
                leafnode,
                retrospective_searched,
                retroflips,
                node_limit,
            ) {
                SearchResult::Found => return SearchResult::Found,
                SearchResult::Unknown => return SearchResult::Unknown,
                SearchResult::NotFound => {}
            }
        }
    }

    // Iterate candidate opponent stones (excluding initial 4 centers)
    let mut b = board.opponent & !0x0000_0018_1800_0000u64;
    if b == 0 {
        return SearchResult::NotFound;
    }

    // Ensure workspace for current disc count
    if retroflips.len() <= num_disc {
        retroflips.resize(num_disc + 1, [0u64; 10_000]);
    }

    while b != 0 {
        let index = b.trailing_zeros();
        b &= b - 1;

        // Enumerate possible flip-sets if the last move was at `index`
        let num = retrospective_flip(
            index,
            board.player,
            board.opponent,
            &mut retroflips[num_disc],
        );
        for i in 1..num {
            let flipped = retroflips[num_disc][i];
            debug_assert!(flipped != 0);

            let prev = Board {
                player: board.opponent ^ (flipped | (1u64 << index)),
                opponent: board.player ^ flipped,
            };

            match retrospective_search_limited(
                &prev,
                false,
                discs,
                leafnode,
                retrospective_searched,
                retroflips,
                node_limit,
            ) {
                SearchResult::Found => return SearchResult::Found,
                SearchResult::Unknown => return SearchResult::Unknown,
                SearchResult::NotFound => {}
            }
        }
    }

    SearchResult::NotFound
}

/// pos は opponent が直前に置いた位置 (0..=63)。
/// 「直前の着手が pos だった」と仮定したときに、
/// その着手であり得る “ひっくり返り集合” を result に列挙して個数を返す。
/// 返り値が非ゼロのとき `result[0] == 0`（便宜上）。反復時は 1 から使うこと。
pub fn retrospective_flip(
    pos: u32,
    _player: u64,
    opponent: u64,
    result: &mut [u64; 10_000],
) -> usize {
    assert!(pos < 64);
    assert!(((1u64 << pos) & opponent) != 0);
    // 中央 4 マスではない（問題文どおり）
    assert!(((1u64 << pos) & 0x0000_0018_1800_0000u64) == 0);

    let xpos = (pos % 8) as i32;
    let ypos = (pos / 8) as i32;

    let mut answer: usize = 0;

    // ユーティリティ：answer==0 のとき初期化、それ以外は直積結合
    #[inline]
    fn add_direction_sets(
        answer: &mut usize,
        result: &mut [u64; 10_000],
        acc_bits_seq: impl Iterator<Item = u64>,
    ) {
        if *answer == 0 {
            // 初回：result[0] = 0、以後は累積ORで 1..n-1 を埋める
            result[0] = 0;
            *answer = 1;
            for bits in acc_bits_seq {
                debug_assert!(*answer < result.len());
                result[*answer] = result[*answer - 1] | bits;
                *answer += 1;
            }
        } else {
            // 2 回目以降：既存 0..old_answer-1 に対して各累積方向 bits を OR した新要素を追加
            let old_answer = *answer;
            let mut direction: u64 = 0;
            for bits in acc_bits_seq {
                direction |= bits;
                for j in 0..old_answer {
                    debug_assert!(*answer < result.len());
                    result[*answer] = result[j] | direction;
                    *answer += 1;
                }
            }
        }
    }

    // 上方向（-8）
    if ypos >= 2 {
        let mut length = 0;
        loop {
            let next = pos as i32 - ((length + 1) * 8);
            if next < 0 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == ypos {
                break;
            }
        }
        if length >= 2 {
            // 1..=length-1 個を候補として累積
            let seq = (1..length).map(|i| 1u64 << (pos - (i as u32 * 8)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 下方向（+8）
    if ypos < 6 {
        let mut length = 0;
        loop {
            let next = pos as i32 + ((length + 1) * 8);
            if next > 63 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == 7 - ypos {
                break;
            }
        }
        if length >= 2 {
            let seq = (1..length).map(|i| 1u64 << (pos + (i as u32 * 8)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 右方向（-1）
    if xpos >= 2 {
        let mut length = 0;
        loop {
            let next = pos as i32 - (length + 1);
            if next < 0 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == xpos {
                break;
            }
        }
        if length >= 2 {
            let seq = (1..length).map(|i| 1u64 << (pos - i as u32));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 左方向（+1）
    if xpos < 6 {
        let mut length = 0;
        loop {
            let next = pos as i32 + (length + 1);
            if next > 63 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == 7 - xpos {
                break;
            }
        }
        if length >= 2 {
            let seq = (1..length).map(|i| 1u64 << (pos + i as u32));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 右上（-9）
    if xpos >= 2 && ypos >= 2 {
        let mut length = 0;
        loop {
            let next = pos as i32 - ((length + 1) * 9);
            if next < 0 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == min(xpos, ypos) {
                break;
            }
        }
        if length >= 2 {
            let seq = (1..length).map(|i| 1u64 << (pos - (i as u32 * 9)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 左下（+9）
    if xpos < 6 && ypos < 6 {
        let mut length = 0;
        loop {
            let next = pos as i32 + ((length + 1) * 9);
            if next > 63 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == min(7 - xpos, 7 - ypos) {
                break;
            }
        }
        if length >= 2 {
            let seq = (1..length).map(|i| 1u64 << (pos + (i as u32 * 9)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 左上（-7）
    if xpos < 6 && ypos >= 2 {
        let mut length = 0;
        loop {
            let next = pos as i32 - ((length + 1) * 7);
            if next < 0 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == min(7 - xpos, ypos) {
                break;
            }
        }
        if length >= 2 {
            let seq = (1..length).map(|i| 1u64 << (pos - (i as u32 * 7)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 右下（+7）
    if xpos >= 2 && ypos < 6 {
        let mut length = 0;
        loop {
            let next = pos as i32 + ((length + 1) * 7);
            if next > 63 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == min(xpos, 7 - ypos) {
                break;
            }
        }
        if length >= 2 {
            let seq = (1..length).map(|i| 1u64 << (pos + (i as u32 * 7)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    answer
}

/// C++ の retrospective_search を、グローバル無しで移植。
/// - `discs`: DISCS に相当する閾値
/// - `leafnode`: 事前に収集済みのユニーク局面集合（しきい値以上で合法手があるもの）
/// - `retrospective_searched`: 既訪問ユニーク局面
/// - `retroflips`: ディスク数ごとに使い回す作業バッファ（長さ 10_000 の配列を入れておく）
///   インデックスは `num_disc as usize` を想定。必要に応じて拡張する。
pub fn retrospective_search(
    board: &Board,
    from_pass: bool,
    discs: i32,
    leafnode: &HashSet<[u64; 2]>,
    retrospective_searched: &mut HashSet<[u64; 2]>,
    retroflips: &mut Vec<[u64; 10_000]>,
) -> bool {
    let uni = board.unique();
    let num_disc = board.popcount() as usize;

    // しきい値以下なら leafnode に含まれているか確認
    if (num_disc as i32) <= discs {
        if leafnode.contains(&uni) {
            println!("info: found unique board in leafnodes:");
            println!("unique player = {}", uni[0]);
            println!("unique opponent = {}", uni[1]);
            println!("board player = {}", board.player);
            println!("board opponent = {}", board.opponent);
            return true;
        }
        return false;
    }

    // 再訪防止
    if !retrospective_searched.insert(uni) {
        return false;
    }
    if retrospective_searched.len() > 0x20000000 {
        println!("Memory overflow");
        return true;
    }

    // 8 近傍で連結でなければ打ち切り
    let occupied = board.player | board.opponent;
    if !is_connected(occupied) {
        return false;
    }

    if !check_seg3(occupied) {
        return false;
    }
    // let line = board.to_string();
    // if !is_sat_ok(0, &line).unwrap() {
    //     return false;
    // }

    // パス遡り（from_pass=false かつ 相手に合法手が無い場合）
    if !from_pass {
        if get_moves(board.opponent, board.player) == 0 {
            let prev = Board {
                player: board.opponent,
                opponent: board.player,
            };
            if retrospective_search(
                &prev,
                true,
                discs,
                leafnode,
                retrospective_searched,
                retroflips,
            ) {
                println!("pass");
                return true;
            }
        }
    }

    // 相手石（中央4マス以外）を候補として走査
    let mut b = board.opponent & !0x0000_0018_1800_0000u64;
    if b == 0 {
        return false;
    }

    // retroflips[num_disc] を使うので、足りなければ拡張
    if retroflips.len() <= num_disc {
        retroflips.resize(num_disc + 1, [0u64; 10_000]);
    }

    // （デバッグ用カウンタ：C++ と同様に保持するが使っていない）
    let mut _searched: i32 = 0;

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

            if retrospective_search(
                &prev,
                false,
                discs,
                leafnode,
                retrospective_searched,
                retroflips,
            ) {
                println!("{}", index);
                return true;
            }
        }
    }

    false
}
