use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fs::File;
use std::io::Write;
use std::io::{self, BufRead, BufReader};
use std::io::{Error, ErrorKind};
//use rustsat::instances::SatInstance;
//use rustsat::instances::fio::dimacs;
use rustsat::solvers::Solve;
//use rustsat_kissat::Kissat;
//use rustsat::types::{Lit, Var, Clause};
use rustsat::types::{Clause, Lit};
// use rustsat::instances::{BasicVarManager, CnfFormula};
use rustsat::instances::Cnf;

use std::cmp::Ordering;

/**
 * edax-reversi
 *
 * https://github.com/abulmo/edax-reversi
 *
 * @date 1998 - 2017
 * @author Richard Delorme
 * @version 4.4
 */

/**
 * edax-reversi-AVX
 *
 * https://github.com/okuhara/edax-reversi-AVX
 *
 * @date 1998 - 2018
 * @author Toshihiko Okuhara
 * @version 4.4
 */

/**
 * retrospective-dfs-reversi
 *
 * https://github.com/eukaryo/retrospective-dfs-reversi
 *
 * @date 2020
 * @author Hiroki Takizawa
 */

/// translated with ChatGPT 4o
#[inline]
fn bit_scan_forward64(mask: u64) -> Option<u32> {
    if mask == 0 {
        None
    } else {
        Some(mask.trailing_zeros())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Board {
    pub player: u64,
    pub opponent: u64,
}

impl Board {
    pub fn new(player: u64, opponent: u64) -> Self {
        Self { player, opponent }
    }

    pub fn empty() -> Self {
        Self {
            player: 0,
            opponent: 0,
        }
    }

    fn transpose(b: u64) -> u64 {
        let mut b = b;
        let mut t;

        t = (b ^ (b >> 7)) & 0x00aa00aa00aa00aa;
        b ^= t ^ (t << 7);
        t = (b ^ (b >> 14)) & 0x0000cccc0000cccc;
        b ^= t ^ (t << 14);
        t = (b ^ (b >> 28)) & 0x00000000f0f0f0f0;
        b ^ (t ^ (t << 28))
    }

    fn vertical_mirror(b: u64) -> u64 {
        let mut b = b;
        b = ((b >> 8) & 0x00FF00FF00FF00FF) | ((b << 8) & 0xFF00FF00FF00FF00);
        b = ((b >> 16) & 0x0000FFFF0000FFFF) | ((b << 16) & 0xFFFF0000FFFF0000);
        ((b >> 32) & 0x00000000FFFFFFFF) | ((b << 32) & 0xFFFFFFFF00000000)
    }

    fn horizontal_mirror(b: u64) -> u64 {
        let mut b = b;
        b = ((b >> 1) & 0x5555555555555555) | ((b << 1) & 0xAAAAAAAAAAAAAAAA);
        b = ((b >> 2) & 0x3333333333333333) | ((b << 2) & 0xCCCCCCCCCCCCCCCC);
        ((b >> 4) & 0x0F0F0F0F0F0F0F0F) | ((b << 4) & 0xF0F0F0F0F0F0F0F0)
    }

    fn board_check(board: [u64; 2]) {
        if board[0] & board[1] != 0 {
            panic!("Two discs on the same square?");
        }
        if (board[0] | board[1]) & 0x0000001818000000 != 0x0000001818000000 {
            panic!("Empty center?");
        }
    }

    fn board_symmetry(&self, s: i32, sym: &mut [u64; 2]) {
        let mut board = [self.player, self.opponent];

        if s & 1 != 0 {
            board[0] = Self::horizontal_mirror(board[0]);
            board[1] = Self::horizontal_mirror(board[1]);
        }
        if s & 2 != 0 {
            board[0] = Self::vertical_mirror(board[0]);
            board[1] = Self::vertical_mirror(board[1]);
        }
        if s & 4 != 0 {
            board[0] = Self::transpose(board[0]);
            board[1] = Self::transpose(board[1]);
        }

        *sym = board;
        Self::board_check(*sym);
    }

    pub fn popcount(&self) -> u32 {
        self.player.count_ones() + self.opponent.count_ones()
    }

    pub fn unique(&self) -> [u64; 2] {
        let mut tmp = [0u64, 0u64];
        let mut answer = [self.player, self.opponent];

        for i in 1..8 {
            self.board_symmetry(i, &mut tmp);
            if tmp < answer {
                answer = tmp;
            }
        }

        Self::board_check(answer);
        answer
    }

    pub fn initial() -> Self {
        Self::new(0x0000000810000000, 0x0000001008000000)
    }
    pub fn to_string(&self) -> String {
        let mut ans: Vec<char> = vec![];
        for y in 0..8 {
            for x in 0..8 {
                let m = 1 << (y * 8 + x);
                if self.player & m != 0 {
                    ans.push('X');
                } else if self.opponent & m != 0 {
                    ans.push('O');
                } else {
                    ans.push('-');
                }
            }
        }
        ans.into_iter().collect()
    }
    pub fn show(&self) -> String {
        let mut ans: Vec<char> = vec![];
        for y in 0..8 {
            for x in 0..8 {
                let m = 1 << (y * 8 + x);
                if self.player & m != 0 {
                    ans.push('X');
                } else if self.opponent & m != 0 {
                    ans.push('O');
                } else {
                    ans.push('-');
                }
            }
            ans.push('\n');
        }
        ans.into_iter().collect()
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

// OrdとPartialOrdを実装（C++のoperator <などに相当）
impl PartialOrd for Board {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Board {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.player.cmp(&other.player) {
            Ordering::Equal => self.opponent.cmp(&other.opponent),
            ord => ord,
        }
    }
}
const DIRS: [i32; 4] = [1, 8, 9, 7];
const MASKS: [u64; 4] = [
    0x7e7e7e7e7e7e7e7e, // horizontal
    0xffffffffffffff00, // vertical
    0x7e7e7e7e7e7e7e00, // diag \
    0x007e7e7e7e7e7e7e, // diag /
];

#[inline(always)]
const fn not_a_file() -> u64 {
    0xFEFE_FEFE_FEFE_FEFE
}
#[inline(always)]
const fn not_h_file() -> u64 {
    0x7F7F_7F7F_7F7F_7F7F
}
#[inline(always)]
//const fn not_rank_1() -> u64 { 0xFF00_FFFF_FFFF_FFFF }
const fn not_rank_1() -> u64 {
    0xFFFF_FFFF_FFFF_FF00
}
#[inline(always)]
const fn not_rank_8() -> u64 {
    0x00FF_FFFF_FFFF_FFFF
}

#[inline(always)]
fn east(x: u64) -> u64 {
    (x << 1) & not_a_file()
}
#[inline(always)]
fn west(x: u64) -> u64 {
    (x >> 1) & not_h_file()
}
#[inline(always)]
fn north(x: u64) -> u64 {
    (x << 8) & not_rank_1()
}
#[inline(always)]
fn south(x: u64) -> u64 {
    (x >> 8) & not_rank_8()
}
#[inline(always)]
fn ne(x: u64) -> u64 {
    (x << 9) & (not_a_file() & not_rank_1())
}
#[inline(always)]
fn nw(x: u64) -> u64 {
    (x << 7) & (not_h_file() & not_rank_1())
}
#[inline(always)]
fn se(x: u64) -> u64 {
    (x >> 7) & (not_a_file() & not_rank_8())
}
#[inline(always)]
fn sw(x: u64) -> u64 {
    (x >> 9) & (not_h_file() & not_rank_8())
}

/// 1方向に対する「はさみ取り」判定。はさめるならその方向の反転集合を返す。
#[inline(always)]
fn ray_flips<F>(move_bb: u64, player: u64, opponent: u64, step: F) -> u64
where
    F: Fn(u64) -> u64,
{
    // 直後の1マスへ進める
    let mut x = step(move_bb);
    let mut flips = 0u64;

    // 連続する相手石を収集
    while x != 0 && (x & opponent) != 0 {
        flips |= x;
        x = step(x);
    }

    // その先に自石があるなら挟めている→反転成立
    if x & player != 0 {
        flips
    } else {
        0
    }
}

/// 与えられた pos に打ったときにひっくり返る相手石の集合を返す（打った石は含まない）
pub fn flip(pos: usize, player: u64, opponent: u64) -> u64 {
    debug_assert!(pos < 64);
    let move_bb = 1u64 << pos;

    // 盤上に既に石があるマスなら反転なし（安全のため）
    if (move_bb & (player | opponent)) != 0 {
        return 0;
    }

    ray_flips(move_bb, player, opponent, east)
        | ray_flips(move_bb, player, opponent, west)
        | ray_flips(move_bb, player, opponent, north)
        | ray_flips(move_bb, player, opponent, south)
        | ray_flips(move_bb, player, opponent, ne)
        | ray_flips(move_bb, player, opponent, nw)
        | ray_flips(move_bb, player, opponent, se)
        | ray_flips(move_bb, player, opponent, sw)
}

pub fn get_moves(player: u64, opponent: u64) -> u64 {
    let mut moves = 0u64;
    for pos in 0..64 {
        let bit = 1u64 << pos;
        if bit & (player | opponent) == 0 {
            if flip(pos, player, opponent) != 0 {
                moves |= bit;
            }
        }
    }
    moves
}

fn xy2sq(x: i32, y: i32) -> usize {
    (y * 8 + x) as usize
}
struct VarMaker {
    count: i32,
}
impl VarMaker {
    pub fn new() -> Self {
        VarMaker { count: 0 }
    }
    fn mkVar(&mut self) -> i32 {
        self.count += 1;
        self.count
    }
    fn count(&self) -> usize {
        self.count as usize
    }
}

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
//
//
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
use std::cmp::min;

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
        mut acc_bits_seq: impl Iterator<Item = u64>,
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
    let line = board.to_string();
    if !is_sat_ok(0, &line).unwrap() {
        return false;
    }

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

const DXYS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];
fn solve_by_kissat(
    index: usize,
    vs: &Vec<Vec<i32>>,
    numVar: usize,
    comment: &HashMap<usize, String>,
) -> bool {
    let mut solver = rustsat_kissat::Kissat::default();
    let mut cnf = Cnf::new();
    for line in vs {
        let mut clause = Clause::new();
        for i in 0..line.len() {
            if line[i] > 0 {
                clause.add(Lit::positive(line[i] as u32));
            } else {
                clause.add(Lit::negative((-line[i]) as u32));
            }
        }
        cnf.add_clause(clause);
    }
    solver.add_cnf(cnf);
    let result = match solver.solve() {
        Ok(res) => res,
        Err(e) => return false,
        //rustsat::solvers::SolverResult::Sat => println!("SAT: 解あり"),
        //rustsat::solvers::SolverResult::Unsat => println!("UNSAT: 解なし"),
        //rustsat::solvers::SolverResult::Unknown => println!("UNKNOWN: 解けませんでした"),
    };
    result == rustsat::solvers::SolverResult::Sat
    //eprintln!("result={:?}", result);
    //Ok(())
}
fn output_cnf(
    index: usize,
    vs: &Vec<Vec<i32>>,
    numVar: usize,
    comment: &HashMap<usize, String>,
) -> Result<(), Error> {
    let filename = format!("{}.cnf", index);
    let mut file = File::create(&filename)?;
    for (i, line) in comment.iter() {
        writeln!(file, "c Var_{}, {}", i, line.clone());
    }
    writeln!(file, "p cnf {} {}", numVar, vs.len());
    for line in vs {
        write!(file, "c ");
        for i in 0..line.len() {
            if i > 0 {
                write!(file, " ");
            }
            if line[i] > 0 {
                let v = line[i] as usize;
                write!(file, "{}", comment.get(&v).unwrap().to_string());
            } else {
                let v = (-line[i]) as usize;
                write!(file, "-{}", comment.get(&v).unwrap().to_string());
            }
        }
        writeln!(file, "");
        for i in 0..line.len() {
            if i > 0 {
                write!(file, " ");
            }
            write!(file, "{}", line[i]);
        }
        writeln!(file, " 0");
    }
    writeln!(file, "");
    // Ok(())
    Err(Error::new(ErrorKind::Other, "one cnf file only"))
}
fn is_sat_ok(index: usize, line: &String) -> Result<bool, Error> {
    eprintln!("line={}", line);
    let cs: Vec<char> = line.chars().collect();
    if cs.len() != 64 {
        return Err(Error::new(
            ErrorKind::Other,
            "length is not 64 format error",
        ));
    }
    let mut sqi: Vec<usize> = vec![];
    let mut sqo: Vec<usize> = vec![];
    let mut sqall: Vec<usize> = vec![];
    let mut vm = VarMaker::new();
    let mut in_sqo: Vec<bool> = vec![false; 64];
    for y in 0..8 {
        for x in 0..8 {
            let sq = xy2sq(x, y);
            if cs[sq] != '-' {
                sqall.push(sq);
                if 3 <= x && x <= 4 && 3 <= y && y <= 4 {
                    sqi.push(sq);
                } else {
                    sqo.push(sq);
                    in_sqo[sq] = true;
                }
            }
        }
    }
    if sqi.len() != 4 {
        return Err(Error::new(ErrorKind::Other, "empty squares in center 2x2"));
    }
    let sq33 = xy2sq(3, 3);
    // First[sq][col] : sqに最初に置かれる石がcolかどうかを表す論理変数
    let mut First: Vec<Vec<i32>> = vec![vec![0; 2]; 64];
    // Flip[sq][col] : [(sq', col, d, len)], sqをcolにflipするflip全体
    let mut Flip: Vec<Vec<Vec<(usize, usize, usize, usize)>>> = vec![vec![vec![]; 2]; 64];
    // Set[sq][col] : [(sq', col, d, len)], flipに加えて First[sq][col] に対応する(sq, col, 0, 0) も含む
    let mut Set: Vec<Vec<Vec<(usize, usize, usize, usize)>>> = vec![vec![vec![]; 2]; 64];
    // Base[sq][col] : [(sq', col, d, len)], sqがcolであることを利用してcolにflipするflip
    let mut Base: Vec<Vec<Vec<(usize, usize, usize, usize)>>> = vec![vec![vec![]; 2]; 64];
    // F[(sq, col, d, len)] : flip (sq, col, d, len) から論理変数への変換
    let mut F: HashMap<(usize, usize, usize, usize), i32> = HashMap::new();
    let v_sq33 = vm.mkVar();
    let mut comment: HashMap<usize, String> = HashMap::new();
    comment.insert(vm.count(), format!("Square33").to_string());
    for &sq in &sqall {
        let v = if in_sqo[sq] {
            comment.insert(vm.count() + 1, format!("Square_{}", sq).to_string());
            vm.mkVar()
        } else {
            v_sq33 * if sq / 8 == sq % 8 { 1 } else { -1 }
        };
        for col in 0..2 {
            let t = (sq, col, 0, 0);
            let v1 = if col == 0 { v } else { -v };
            First[sq][col] = v1;
            F.insert(t, v1);
            Set[sq][col].push(t);
        }
    }
    let mut Cmp: Vec<Vec<i32>> = vec![vec![0; 64]; 64];
    let mut s: Vec<Vec<i32>> = vec![];
    // eprintln!("sqo.len() = {}", sqo.len());
    for &sq in &sqo {
        for &sq1 in &sqo {
            if sq != sq1 {
                Cmp[sq][sq1] = vm.mkVar();
                comment.insert(vm.count(), format!("Cmp[{}][{}]", sq, sq1).to_string());
            }
        }
    }
    for &sq in &sqo {
        for &sq1 in &sqo {
            if sq != sq1 {
                if sq < sq1 {
                    // sq < sq1 かつ sq1 < sq となることはない．
                    s.push(vec![-Cmp[sq][sq1], -Cmp[sq1][sq]]);
                }
                for &sq2 in &sqo {
                    if sq2 != sq && sq2 != sq1 {
                        // 順序関係には推移律が成り立つ
                        s.push(vec![-Cmp[sq][sq2], -Cmp[sq2][sq1], Cmp[sq][sq1]]);
                    }
                }
            }
        }
    }
    //eprintln!("end of Cmp, s.len()={}", s.len());
    for &sq in &sqo {
        let x = (sq % 8) as i32;
        let y = (sq / 8) as i32;
        for col in 0..2 {
            let mut ps: Vec<i32> = vec![]; // sqにcolの石を置くすべてのflip
            for (d, (dx, dy)) in DXYS.iter().enumerate() {
                let mut sqs: Vec<usize> = vec![];
                let mut rl = 1;
                let mut x1 = x + dx;
                let mut y1 = y + dy;
                let mut samedir: Vec<i32> = vec![];
                while 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && cs[xy2sq(x1, y1)] != '-' {
                    rl += 1;
                    let sq1 = xy2sq(x1, y1);
                    if rl >= 3 {
                        let t = (sq, col, d, rl);
                        let v = vm.mkVar();
                        comment.insert(vm.count(), format!("{:?}", t).to_string());
                        F.insert(t, v);
                        ps.push(v);
                        samedir.push(v);
                        for &sq2 in &sqs {
                            Flip[sq2][col].push(t);
                            Set[sq2][col].push(t);
                            if in_sqo[sq2] {
                                s.push(vec![-v, Cmp[sq2][sq]]);
                            }
                        }
                        Base[sq1][col].push(t);
                        if in_sqo[sq1] {
                            s.push(vec![-v, Cmp[sq1][sq]]);
                        }
                    }
                    sqs.push(sq1);
                    x1 += dx;
                    y1 += dy;
                }
                for i in 1..samedir.len() {
                    for j in 0..i {
                        s.push(vec![-samedir[i], -samedir[j]]);
                    }
                }
            }
            let mut line = vec![-First[sq][col]];
            for &f in &ps {
                // First[sq][1 - col] なら，psの中のflipはFalseになる．
                s.push(vec![-First[sq][1 - col], -f]);
                line.push(f);
            }
            // First[sq][col] なら，psの中のいずれかのflipがTrue
            s.push(line);
        }
    }
    //    eprintln!("end of First, s.len()={}", s.len());

    // Last
    // let mut Last: HashMap<(usize, (usize, usize, usize, usize)), i32> = HashMap::new();
    for &sq in &sqall {
        let last_c = if cs[sq] == 'X' { 1 } else { 0 };
        let mut vs = vec![];
        for &t in &Set[sq][last_c] {
            let v = *F.get(&t).unwrap();
            let v1 = vm.mkVar();
            comment.insert(vm.count(), format!("Last[{:?}]", t).to_string());
            vs.push(v1);
            s.push(vec![-v1, v]);
            for col in 0..2 {
                for &t1 in &Flip[sq][col] {
                    if t.0 != t1.0 && in_sqo[t.0] && in_sqo[t1.0] {
                        s.push(vec![-v1, -F.get(&t1).unwrap(), Cmp[t1.0][t.0]]);
                    }
                }
            }
        }
        for i in 1..vs.len() {
            for j in 0..i {
                s.push(vec![-vs[i], -vs[j]]);
            }
        }
        if vs.len() > 0 {
            s.push(vs);
        }
    }
    //eprintln!("end of Last, s.len()={}", s.len());
    // Before
    let mut Before: HashMap<
        (
            usize,
            (usize, usize, usize, usize),
            (usize, usize, usize, usize),
        ),
        i32,
    > = HashMap::new();
    for &sq in &sqo {
        for col in 0..2 {
            for &t in &Set[sq][col] {
                for &t1 in &Flip[sq][1 - col] {
                    if t.0 != t1.0 {
                        Before.insert((sq, t, t1), vm.mkVar());
                        comment.insert(
                            vm.count(),
                            format!("Before[({}, {:?}, {:?})]", sq, t, t1).to_string(),
                        );
                    }
                }
                for &t1 in &Base[sq][col] {
                    if t.0 != t1.0 {
                        Before.insert((sq, t, t1), vm.mkVar());
                        comment.insert(
                            vm.count(),
                            format!("Before[({}, {:?}, {:?})]", sq, t, t1).to_string(),
                        );
                    }
                }
            }
        }
    }
    for ((sq, t1, t2), v) in Before.iter() {
        if t1.3 != 0 || in_sqo[*sq] {
            s.push(vec![-v, Cmp[t1.0][t2.0]]);
        }
        s.push(vec![-v, *F.get(&t1).unwrap()]);
        s.push(vec![-v, *F.get(&t2).unwrap()]);
    }
    for &sq in &sqo {
        // let last_c = if cs[sq] == 'X' {1} else {0};
        for col in 0..2 {
            for &t1 in &Flip[sq][1 - col] {
                let mut vs: Vec<i32> = vec![-*F.get(&t1).unwrap()];
                for &t in &Set[sq][col] {
                    if t1.0 == t.0 {
                        continue;
                    }
                    vs.push(*Before.get(&(sq, t, t1)).unwrap());
                }
                s.push(vs);
            }
            for &t1 in &Base[sq][col] {
                let mut vs: Vec<i32> = vec![-*F.get(&t1).unwrap()];
                for &t in &Set[sq][col] {
                    if t1.0 == t.0 {
                        continue;
                    }
                    vs.push(*Before.get(&(sq, t, t1)).unwrap());
                }
                s.push(vs);
            }
        }
    }
    //output_cnf(index, &s, vm.count(), &comment)
    let ans = solve_by_kissat(index, &s, vm.count(), &comment);

    Ok(ans)
}

// const DISCMAX: i32 = 15;
const DISCMAX: i32 = 5;
fn process_line(
    index: usize,
    line: &String,
    searched: &HashSet<[u64; 2]>,
    leafnode: &HashSet<[u64; 2]>,
    retrospective_searched: &mut HashSet<[u64; 2]>,
    retroflips: &mut Vec<[u64; 10_000]>,
) -> Result<bool, Error> {
    let mut player: u64 = 0;
    let mut opponent: u64 = 0;
    for (i, c) in line.chars().enumerate() {
        if c == 'X' {
            player |= 1_u64 << i;
        } else if c == 'O' {
            opponent |= 1_u64 << i;
        }
    }
    let bb = Board::new(player, opponent);
    let line = bb.to_string();
    if !is_sat_ok(0, &line).unwrap() {
        return Ok(false);
    } else {
        return Ok(true);
    }
    let uni = bb.unique();
    if searched.contains(&uni) || leafnode.contains(&uni) {
        return Ok(true);
    }
    retrospective_searched.clear();
    retroflips.resize(bb.popcount() as usize + 1, [0u64; 10_000]);
    let mut ans = false;
    if retrospective_search(
        &bb,
        false,
        DISCMAX,
        leafnode,
        retrospective_searched,
        retroflips,
    ) {
        println!("OK: {}", bb.to_string());
        println!("len(searched)={}", retrospective_searched.len());
        ans = true;
    } else {
        println!("NG: {}", bb.to_string());
    }
    Ok(ans)
}
fn process_file(filename: String) -> io::Result<()> {
    let file = File::open(&filename)?;
    let reader = BufReader::new(file);
    let mut okfile = File::create("sat_OK.txt")?;
    let mut ngfile = File::create("sat_NG.txt")?;
    let mut searched: HashSet<[u64; 2]> = HashSet::new();
    let mut leafnode: HashSet<[u64; 2]> = HashSet::new();
    for i in 5..=DISCMAX {
        searched.clear();
        leafnode.clear();
        let discs = i;
        let board = Board::initial();
        search(&board, &mut searched, &mut leafnode, discs);
        println!(
            "discs = {}: internal nodes = {}, leaf nodes = {}",
            i,
            searched.len(),
            leafnode.len()
        );
    }
    let mut retrospective_searched: HashSet<[u64; 2]> = HashSet::new();
    let mut retroflips: Vec<[u64; 10_000]> = vec![];
    // 1行ずつ読み込んで処理する
    for (index, line_result) in reader.lines().enumerate() {
        let line = line_result?; // Result<String> なのでアンラップ
        match process_line(
            index,
            &line,
            &searched,
            &leafnode,
            &mut retrospective_searched,
            &mut retroflips,
        ) {
            Ok(res) => {
                if res {
                    println!("{}", line);
                    writeln!(okfile, "{}", line);
                } else {
                    writeln!(ngfile, "{}", line);
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    }
    Ok(())
}
fn main() {
    let args: Vec<String> = env::args().collect();
    for (i, args) in args.iter().enumerate() {
        if i > 0 {
            if let Err(e) = process_file(args.to_string()) {
                eprintln!("Error: {}", e);
            }
        }
        println!("argv[{}] : {}", i, args);
    }
}
