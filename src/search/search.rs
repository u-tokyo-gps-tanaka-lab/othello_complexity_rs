use crate::othello::{flip, get_moves, Board, DXYS};
use crate::prunings::check_occupancy::{check_occupancy, occupancy_order};

use std::cmp::min;
use std::collections::HashSet;

pub struct Btable {
    cache_size: usize,
    table: Vec<[u64; 2]>,
    cache: HashSet<[u64; 2]>,
}

impl Btable {
    pub fn new(table_size: usize, cache_size: usize) -> Self {
        Btable {
            cache_size: cache_size,
            table: Vec::with_capacity(table_size),
            cache: HashSet::new(),
        }
    }
    pub fn clear(&mut self) {
        self.table.clear();
        self.cache.clear();
    }
    pub fn len(&self) -> usize {
        let ans = self.cache.len() + self.table.len();
        ans
    }
    fn insert(&mut self, uni: [u64; 2]) -> bool {
        if self.cache.contains(&uni) {
            return false;
        }
        if let Ok(_) = self.table.binary_search(&uni) {
            return false;
        }
        self.cache.insert(uni);
        if self.cache.len() >= self.cache_size {
            if self.table.len() + self.cache.len() > self.table.capacity() {
                self.cache.clear();
                return true;
            }
            let mut c2v: Vec<[u64; 2]> = self.cache.iter().map(|x| *x).collect();
            self.cache.clear();
            c2v.sort();
            let mut i = self.table.len();
            let mut j = c2v.len();
            self.table.resize(i + j, [0u64; 2]);
            for k in (0..(i + j)).rev() {
                if j == 0 || (i > 0 && self.table[i - 1] >= c2v[j - 1]) {
                    self.table[k] = self.table[i - 1];
                    i -= 1;
                } else {
                    self.table[k] = c2v[j - 1];
                    j -= 1;
                }
            }
        }
        return true;
    }
}

#[allow(dead_code)]
fn mask_to_moves(m: u64) -> String {
    let mut ans: Vec<String> = vec!["[".to_string()];
    for i in 0..64 {
        if m & (1 << i) != 0 {
            let y = i / 8;
            let x = i % 8;
            ans.push(format!("({}, {})", x, y));
        }
    }
    ans.push("]".to_string());
    ans.join(",")
}

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
pub fn is_connected(b: u64) -> bool {
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

/// 下記の2条件によって到達不能な局面を検出する
/// 矛盾が生じる（到達不能）ならfalse, 無矛盾ならtrue
///
/// cond 1:
/// ある石から見て、8方向のどの方向にも連続した3つ以上の石が存在しない場合、その石を置いた際にflip操作が起こらなかったことになり、矛盾する。
/// 従って局面が初期配置から到達可能であるためには、全ての石について8方向のうち必ず1つ以上の方向で3つ以上の石が連続している必要がある
///
/// cond 2:
/// 局面 $s$ について、64個の各マスを頂点とし、マス$i$への着手がマス$j$に依存している際に $i$ から $j$ への有向辺を持つ有向グラフ $G_s$ を作成する（ただし $i \neq j$）。
/// $G_s$ に閉路が存在するならば、$G_s$に対応する局面$s$は初期局面から到達不能である。
/// 閉路が存在することは「着手の依存関係に循環がある」ことを意味し、矛盾する。
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

#[inline(always)]
pub fn onebit(x: u8) -> bool {
    x & (x - 1) == 0
}

fn can_put_flip(occupied: u64, order: &[u64; 64]) -> ([u8; 64], [u8; 64]) {
    let mut canput: [u8; 64] = [0; 64];
    let mut canflip: [u8; 64] = [0; 64];
    for y in 0..8 {
        for x in 0..8 {
            let i = y * 8 + x;
            if occupied & (1 << i) == 0 {
                continue;
            }
            let mut ls: [u8; 8] = [0; 8];
            let mut ls1: [u8; 8] = [0; 8];
            let o1 = order[i as usize];
            for (d, (dx, dy)) in DXYS.iter().enumerate() {
                let mut l = 1;
                let mut l1 = 1;
                let mut x1 = x + dx;
                let mut y1 = y + dy;
                let mut i1 = y1 * 8 + x1;
                let mut in_o1 = true;
                while 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && occupied & (1 << i1) != 0 {
                    if in_o1 && o1 & (1 << i1) == 0 {
                        in_o1 = false;
                    }
                    if in_o1 {
                        l1 += 1;
                    }
                    l += 1;
                    x1 += dx;
                    y1 += dy;
                    i1 = y1 * 8 + x1;
                }
                ls[d] = l;
                ls1[d] = l1;
            }
            for d in 0..8 {
                if ls1[d] >= 3 {
                    if !(3 <= x && x <= 4 && 3 <= y && y <= 4) {
                        canput[i as usize] |= 1u8 << d;
                    }
                }
                if d < 4 && ls[d] >= 2 && ls[d + 4] >= 2 {
                    canflip[i as usize] |= 1u8 << d;
                }
            }
        }
    }
    (canput, canflip)
}
/// 盤面が初期配置に到達不能かどうかの粗めのチェック．
pub fn check_seg3_more(player: u64, opponent: u64) -> bool {
    //if !check_seg3_more(player, opponent) {
    //    return false;
    //}

    let occupied = player | opponent;
    let order = occupancy_order(occupied);
    let (canput, canflip) = can_put_flip(occupied, &order);
    let ps = [player, opponent];
    for i in 0..2 {
        let p0 = ps[i];
        for y in 0..8 {
            for x in 0..8 {
                if 3 <= x && x <= 4 && 3 <= y && y <= 4 {
                    continue;
                }
                let i = y * 8 + x;
                if p0 & (1 << i) == 0 {
                    continue;
                }
                if canput[i as usize] == 0 {
                    eprintln!("canput = 0, i={}, x={}, y={}", i, x, y);
                    eprintln!("{}", Board::new(player, opponent).show());
                    panic!("inconsistent");
                }
                // putの方向が1方向で後でflipされた可能性がない．
                if canflip[i as usize] != 0 {
                    continue;
                }
                let mut mask = canput[i as usize];
                let mut mask_count = 0;
                let mut ng_count = 0;
                while mask != 0 {
                    let d = mask.trailing_zeros();
                    mask_count += 1;
                    mask &= mask - 1;
                    let d1 = d & 3;
                    let (dx, dy) = DXYS[d as usize];
                    let di = dy * 8 + dx;
                    // 隣と，その隣のマスがd1方向以外にflipされる可能性がない．
                    for i1 in [i + di, i + di * 2] {
                        if p0 & (1 << i1) == 0 {
                            let f = canflip[i1 as usize];
                            if f == 0 {
                                ng_count += 1;
                                break;
                            }
                            if f & !(1 << d1) == 0 {
                                if i1 == i + di {
                                    ng_count += 1;
                                    break;
                                }
                                let i2 = i + di;
                                if p0 & (1 << i2) != 0 && canflip[i2 as usize] & !(1 << d1) == 0 {
                                    //eprintln!("x, y, dx, dy, i1 = {:?}", (x, y, dx, dy,i1));
                                    //eprintln!("{}", Board::new(player, opponent).show());
                                    ng_count += 1;
                                    break;
                                }
                            }
                        }
                    }
                }
                if mask_count == ng_count {
                    //eprintln!("x, y = {:?}", (x, y));
                    //eprintln!("{}", Board::new(player, opponent).show());
                    return false;
                }
            }
        }
    }
    true
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

/// - `from_pass`: 直前にパスで1手分遡ったか否か
/// - `discs`: 順方向探索の深さ（石数）
/// - `leafnode`: 順方向探索で得たuniqueなleafnodeの集合（しきい値以上で合法手があるもの）
/// - `retrospective_searched`: 既訪問ユニーク局面
/// - `retroflips`: ディスク数ごとに使い回す作業バッファ（長さ 10_000 の配列を入れておく）
///   インデックスは `num_disc as usize` を想定。必要に応じて拡張する。
pub fn retrospective_search(
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
            match retrospective_search(
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

            match retrospective_search(
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
    }

    SearchResult::NotFound
}

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
            for (dx, dy) in DXYS.iter() {
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
