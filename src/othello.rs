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

// translated with ChatGPT 4o

pub const DXYS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];
pub const DIRS: [i32; 4] = [1, 8, 9, 7];

pub const CENTER_MASK: u64 = 0x0000_0018_1800_0000u64; // 4 center squares

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

    pub fn board_symmetry(&self, s: i32, sym: &mut [u64; 2]) {
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

#[inline(always)]
const fn not_a_file() -> u64 {
    0xFEFE_FEFE_FEFE_FEFE
}
#[inline(always)]
const fn not_h_file() -> u64 {
    0x7F7F_7F7F_7F7F_7F7F
}
#[inline(always)]
const fn not_rank_1() -> u64 {
    0xFFFF_FFFF_FFFF_FF00
}
#[inline(always)]
const fn not_rank_8() -> u64 {
    0x00FF_FFFF_FFFF_FFFF
}

#[inline(always)]
pub fn east(x: u64) -> u64 {
    (x << 1) & not_a_file()
}
#[inline(always)]
pub fn west(x: u64) -> u64 {
    (x >> 1) & not_h_file()
}
#[inline(always)]
pub fn north(x: u64) -> u64 {
    (x << 8) & not_rank_1()
}
#[inline(always)]
pub fn south(x: u64) -> u64 {
    (x >> 8) & not_rank_8()
}
#[inline(always)]
pub fn ne(x: u64) -> u64 {
    (x << 9) & (not_a_file() & not_rank_1())
}
#[inline(always)]
pub fn nw(x: u64) -> u64 {
    (x << 7) & (not_h_file() & not_rank_1())
}
#[inline(always)]
pub fn se(x: u64) -> u64 {
    (x >> 7) & (not_a_file() & not_rank_8())
}
#[inline(always)]
pub fn sw(x: u64) -> u64 {
    (x >> 9) & (not_h_file() & not_rank_8())
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

/// ボード検証のエラー型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardValidation {
    /// プレイヤーと相手の石が重なっている
    Overlap,
    /// 中央4マスが埋まっていない
    MissingCenter,
}

/// ボードが有効かどうかを検証する
pub fn validate_board(board: &Board) -> Result<(), BoardValidation> {
    if (board.player & board.opponent) != 0 {
        return Err(BoardValidation::Overlap);
    }
    let occupied = board.player | board.opponent;
    if (occupied & CENTER_MASK) != CENTER_MASK {
        return Err(BoardValidation::MissingCenter);
    }
    Ok(())
}
