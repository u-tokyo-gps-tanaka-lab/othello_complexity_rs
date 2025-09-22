use crate::lib::othello::{flip, get_moves, Board, DXYS};

use std::cmp::min;
use std::collections::HashSet;

use dashmap::DashSet;
use rayon::ThreadPoolBuilder;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

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
            let next = Board::new(board.opponent, board.player);
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
            let next = Board::new(board.opponent, board.player);
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
        let next = Board::new(
            board.opponent ^ flipped,
            board.player ^ (flipped | (1u64 << idx)),
        );
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
    if !is_connected(occupied) {
        return SearchResult::NotFound;
    }
    if !check_seg3(occupied) {
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
            let prev = Board::new(board.opponent, board.player);
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

            let prev = Board::new(
                // 直前に相手が index に置き、flipped が返ったと仮定した局面の 1 手前
                board.opponent ^ (flipped | (1u64 << index)),
                board.player ^ flipped,
            );

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

//--------------------------------------
// 並列パラメータ（必要なら調整）
const PAR_MAX_DEPTH: usize = 6; // この深さまでは spawn を許可
const PAR_MIN_CHILDREN: usize = 4; // 子の数がこの数以上なら分割を検討

#[inline]
fn should_split(depth: usize, children: usize) -> bool {
    depth < PAR_MAX_DEPTH && children >= PAR_MIN_CHILDREN
}

//--------------------------------------
// thread-local retroflips バッファ
thread_local! {
    static TL_RETRO: RefCell<Vec<[u64; 10_000]>> = RefCell::new(Vec::new());
}

//--------------------------------------
// 並列探索用の共有状態
struct ParShared<'a> {
    leafnode: &'a std::collections::HashSet<[u64; 2]>, // 読み取り専用
    visited: &'a DashSet<[u64; 2]>,                    // 既訪問ユニーク局面
    discs: i32,
    node_limit: usize,
    table_limit: usize,
    node_count: &'a AtomicUsize, // 走査ノード数
    node_per_stone: &'a [AtomicUsize; 65],
    done_per_stone: &'a [AtomicUsize; 65],
    table_count: &'a AtomicUsize, // 走査ノード数

    // 早期停止フラグ: 0=進行中, 1=Found, 2=Unknown(上限超過)
    stop: &'a AtomicUsize,
}

//--------------------------------------
// ユーティリティ：スレッドプール初期化（必要なら呼ぶ）
pub fn init_rayon(num_threads: Option<usize>) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let mut b = ThreadPoolBuilder::new();
        if let Some(n) = num_threads {
            b = b.num_threads(n);
        }
        b.build_global().expect("failed to init global rayon pool");
    });
}

//--------------------------------------
// 公開エントリ：並列版 retrospective（シグネチャを分けました）
pub fn retrospective_search_parallel(
    board: &Board,
    from_pass: bool,
    discs: i32,
    leafnode: &std::collections::HashSet<[u64; 2]>,
    node_limit: usize,
    table_limit: usize,
) -> SearchResult {
    let visited = DashSet::new();
    let node_count = AtomicUsize::new(0);
    let table_count = AtomicUsize::new(0);
    let node_per_stone: [AtomicUsize; 65] = std::array::from_fn(|_| AtomicUsize::new(0));
    let done_per_stone: [AtomicUsize; 65] = std::array::from_fn(|_| AtomicUsize::new(0));
    let stop = AtomicUsize::new(0);

    let shared = ParShared {
        leafnode,
        visited: &visited,
        discs,
        node_limit,
        table_limit,
        node_count: &node_count,
        table_count: &table_count,
        node_per_stone: &node_per_stone,
        done_per_stone: &done_per_stone,
        stop: &stop,
    };

    // ルート呼び出し
    let res = par_retro_core(board, from_pass, &shared, 0);
    for i in 0..=64 {
        eprintln!(
            "{}: {} / {}",
            i,
            done_per_stone[i].load(Ordering::Relaxed),
            node_per_stone[i].load(Ordering::Relaxed)
        );
    }
    res
}

//--------------------------------------
// 動的並列コア
fn par_retro_core(board: &Board, from_pass: bool, sh: &ParShared, depth: usize) -> SearchResult {
    // 全体の早期停止を確認
    match sh.stop.load(Ordering::Relaxed) {
        1 => return SearchResult::Found,
        2 => return SearchResult::Unknown,
        _ => {}
    }

    let uni = board.unique();
    let num_disc = board.popcount() as usize;

    // カウンターの変更
    sh.done_per_stone[num_disc].fetch_add(1, Ordering::Relaxed);

    // しきい以下なら leafnode 照合のみ
    if (num_disc as i32) <= sh.discs {
        let r = if sh.leafnode.contains(&uni) {
            SearchResult::Found
        } else {
            SearchResult::NotFound
        };
        if r == SearchResult::Found {
            let _ = sh
                .stop
                .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed);
        }
        return r;
    }

    // 再訪防止
    let n = sh.table_count.fetch_add(1, Ordering::Relaxed) + 1;
    if n < sh.table_limit {
        if !sh.visited.insert(uni) {
            return SearchResult::NotFound;
        }
    } else {
        if sh.visited.contains(&uni) {
            return SearchResult::NotFound;
        }
    }

    // ノード数制限
    let n = sh.node_count.fetch_add(1, Ordering::Relaxed) + 1;
    if n > sh.node_limit {
        // Unknown（上限超過）を全体に通知
        let _ = sh
            .stop
            .compare_exchange(0, 2, Ordering::Relaxed, Ordering::Relaxed);
        return SearchResult::Unknown;
    }

    // 形状フィルタ
    let occupied = board.player | board.opponent;
    if !is_connected(occupied) || !check_seg3(occupied) {
        return SearchResult::NotFound;
    }

    // ---- 子ノード列挙（パス + 直前着手候補からの retroflips）----
    // 1) パス枝（from_pass==false かつ 相手に合法手無し）
    let mut children: Vec<(Board, bool)> = Vec::new(); // (prev_board, from_pass_prev)
    if !from_pass && get_moves(board.opponent, board.player) == 0 {
        children.push((Board::new(board.opponent, board.player), true));
    }

    // 2) 直前着手位置ごとの “可能 flip 集合” 展開
    let b = board.opponent & !0x0000_0018_1800_0000u64;
    if b == 0 && children.is_empty() {
        return SearchResult::NotFound;
    }

    TL_RETRO.with(|tl| {
        let mut retro = tl.borrow_mut();
        if retro.len() <= num_disc {
            retro.resize(num_disc + 1, [0u64; 10_000]);
        }

        let mut bb = b;
        while bb != 0 {
            let index = bb.trailing_zeros();
            bb &= bb - 1;

            let num = retrospective_flip(index, board.player, board.opponent, &mut retro[num_disc]);
            for i in 1..num {
                let flipped = retro[num_disc][i];
                debug_assert!(flipped != 0);
                let prev = Board::new(
                    board.opponent ^ (flipped | (1u64 << index)),
                    board.player ^ flipped,
                );
                children.push((prev, false));
            }
        }
    });

    if children.is_empty() {
        return SearchResult::NotFound;
    }

    //
    let csize = children.len();
    sh.node_per_stone[num_disc - 1].fetch_add(csize, Ordering::Relaxed);
    // ---- 動的に並列 or 直列を選ぶ ----
    if should_split(depth, children.len()) {
        use std::sync::atomic::AtomicUsize;
        let local_best = AtomicUsize::new(SearchResult::NotFound as usize);

        rayon::scope_fifo(|s| {
            // children を消費して所有権を取り出す
            let mut it = children.into_iter();

            // 先頭はこのスレッドで実行
            if let Some((bd0, fp0)) = it.next() {
                let r0 = par_retro_core(&bd0, fp0, sh, depth + 1);
                match r0 {
                    SearchResult::Found => {
                        local_best.store(SearchResult::Found as usize, Ordering::Relaxed);
                        let _ =
                            sh.stop
                                .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed);
                    }
                    SearchResult::Unknown => {
                        if local_best.load(Ordering::Relaxed) == (SearchResult::NotFound as usize) {
                            local_best.store(SearchResult::Unknown as usize, Ordering::Relaxed);
                        }
                    }
                    SearchResult::NotFound => {}
                }
            }

            // 残りはタスクとして spawn（move で所有権を渡す）
            // 共有する参照は、参照値を変数に束ねて、それを move でキャプチャ
            let lb_ref = &local_best;
            let sh_ref = sh;

            for (bd, fp) in it {
                s.spawn_fifo(move |_| {
                    // bd と fp は move 済み（所有）
                    let r = par_retro_core(&bd, fp, sh_ref, depth + 1);

                    match r {
                        SearchResult::Found => {
                            lb_ref.store(SearchResult::Found as usize, Ordering::Relaxed);
                            let _ = sh_ref.stop.compare_exchange(
                                0,
                                1,
                                Ordering::Relaxed,
                                Ordering::Relaxed,
                            );
                        }
                        SearchResult::Unknown => {
                            if lb_ref.load(Ordering::Relaxed) == (SearchResult::NotFound as usize) {
                                lb_ref.store(SearchResult::Unknown as usize, Ordering::Relaxed);
                            }
                        }
                        SearchResult::NotFound => {}
                    }
                });
            }
        });

        match local_best.load(Ordering::Relaxed) {
            x if x == (SearchResult::Found as usize) => SearchResult::Found,
            x if x == (SearchResult::Unknown as usize) => SearchResult::Unknown,
            _ => SearchResult::NotFound,
        }
    } else {
        // 直列分岐はそのまま
        for (bd, fp) in children {
            let r = par_retro_core(&bd, fp, sh, depth + 1);
            match r {
                SearchResult::Found => return SearchResult::Found,
                SearchResult::Unknown => return SearchResult::Unknown,
                SearchResult::NotFound => {}
            }
            match sh.stop.load(Ordering::Relaxed) {
                1 => return SearchResult::Found,
                2 => return SearchResult::Unknown,
                _ => {}
            }
        }
        SearchResult::NotFound
    }
}
