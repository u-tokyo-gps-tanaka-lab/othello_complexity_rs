use std::cmp::Ordering;
use std::marker::PhantomData;

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

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    NorthWest,
    North,
    NorthEast,
}

impl Direction {
    pub const ALL: [Direction; 8] = [
        Direction::East,
        Direction::SouthEast,
        Direction::South,
        Direction::SouthWest,
        Direction::West,
        Direction::NorthWest,
        Direction::North,
        Direction::NorthEast,
    ];
}

pub trait Geometry {
    const WIDTH: usize;
    const HEIGHT: usize;
    const CELL_COUNT: usize = Self::WIDTH * Self::HEIGHT;
    const SYMMETRY_COUNT: usize = 8;

    fn initial() -> (u64, u64);
    fn center_mask() -> u64;
    fn region_mask() -> u64;
    fn bit_by_index(pos: usize) -> u64;
    fn shift(dir: Direction, bb: u64) -> u64;
    fn transpose(bb: u64) -> u64;
    fn vertical_mirror(bb: u64) -> u64;
    fn horizontal_mirror(bb: u64) -> u64;

    fn bit_at(x: usize, y: usize) -> u64 {
        Self::bit_by_index(y * Self::WIDTH + x)
    }

    fn validate(board: [u64; 2]) {
        if board[0] & board[1] != 0 {
            panic!("Two discs on the same square?");
        }
        if (board[0] | board[1]) & Self::center_mask() != Self::center_mask() {
            panic!("Empty center?");
        }
    }

    fn apply_symmetry(sym: usize, board: &mut [u64; 2]) {
        if sym & 1 != 0 {
            board[0] = Self::horizontal_mirror(board[0]);
            board[1] = Self::horizontal_mirror(board[1]);
        }
        if sym & 2 != 0 {
            board[0] = Self::vertical_mirror(board[0]);
            board[1] = Self::vertical_mirror(board[1]);
        }
        if sym & 4 != 0 {
            board[0] = Self::transpose(board[0]);
            board[1] = Self::transpose(board[1]);
        }

        Self::validate(*board);
    }

    fn bit_to_index(bit: u64) -> Option<usize> {
        if bit.count_ones() != 1 || (bit & Self::region_mask()) == 0 {
            return None;
        }
        for pos in 0..Self::CELL_COUNT {
            if Self::bit_by_index(pos) == bit {
                return Some(pos);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Standard8x8;

impl Standard8x8 {
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
}

impl Geometry for Standard8x8 {
    const WIDTH: usize = 8;
    const HEIGHT: usize = 8;

    fn initial() -> (u64, u64) {
        (0x0000000810000000, 0x0000001008000000)
    }

    fn center_mask() -> u64 {
        0x0000001818000000
    }

    fn region_mask() -> u64 {
        0xFFFF_FFFF_FFFF_FFFF
    }

    fn bit_by_index(pos: usize) -> u64 {
        debug_assert!(pos < 64);
        1u64 << pos
    }

    fn shift(dir: Direction, bb: u64) -> u64 {
        match dir {
            Direction::East => (bb << 1) & Self::not_a_file(),
            Direction::SouthEast => (bb >> 7) & (Self::not_a_file() & Self::not_rank_8()),
            Direction::South => (bb >> 8) & Self::not_rank_8(),
            Direction::SouthWest => (bb >> 9) & (Self::not_h_file() & Self::not_rank_8()),
            Direction::West => (bb >> 1) & Self::not_h_file(),
            Direction::NorthWest => (bb << 7) & (Self::not_h_file() & Self::not_rank_1()),
            Direction::North => (bb << 8) & Self::not_rank_1(),
            Direction::NorthEast => (bb << 9) & (Self::not_a_file() & Self::not_rank_1()),
        }
    }

    fn transpose(mut b: u64) -> u64 {
        let mut t;

        t = (b ^ (b >> 7)) & 0x00aa00aa00aa00aa;
        b ^= t ^ (t << 7);
        t = (b ^ (b >> 14)) & 0x0000cccc0000cccc;
        b ^= t ^ (t << 14);
        t = (b ^ (b >> 28)) & 0x00000000f0f0f0f0;
        b ^ (t ^ (t << 28))
    }

    fn vertical_mirror(mut b: u64) -> u64 {
        b = ((b >> 8) & 0x00FF00FF00FF00FF) | ((b << 8) & 0xFF00FF00FF00FF00);
        b = ((b >> 16) & 0x0000FFFF0000FFFF) | ((b << 16) & 0xFFFF0000FFFF0000);
        ((b >> 32) & 0x00000000FFFFFFFF) | ((b << 32) & 0xFFFFFFFF00000000)
    }

    fn horizontal_mirror(mut b: u64) -> u64 {
        b = ((b >> 1) & 0x5555555555555555) | ((b << 1) & 0xAAAAAAAAAAAAAAAA);
        b = ((b >> 2) & 0x3333333333333333) | ((b << 2) & 0xCCCCCCCCCCCCCCCC);
        ((b >> 4) & 0x0F0F0F0F0F0F0F0F) | ((b << 4) & 0xF0F0F0F0F0F0F0F0)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Board<G: Geometry = Standard8x8> {
    pub player: u64,
    pub opponent: u64,
    _geometry: PhantomData<G>,
}

impl<G: Geometry> Board<G> {
    pub fn new(player: u64, opponent: u64) -> Self {
        Self {
            player,
            opponent,
            _geometry: PhantomData,
        }
    }

    pub fn empty() -> Self {
        Self {
            player: 0,
            opponent: 0,
            _geometry: PhantomData,
        }
    }

    fn board_symmetry(&self, s: usize, sym: &mut [u64; 2]) {
        let mut board = [self.player, self.opponent];
        G::apply_symmetry(s, &mut board);
        *sym = board;
    }

    pub fn popcount(&self) -> u32 {
        self.player.count_ones() + self.opponent.count_ones()
    }

    pub fn unique(&self) -> [u64; 2] {
        let mut tmp = [0u64, 0u64];
        let mut answer = [self.player, self.opponent];

        for i in 1..G::SYMMETRY_COUNT {
            self.board_symmetry(i, &mut tmp);
            if tmp < answer {
                answer = tmp;
            }
        }

        G::validate(answer);
        answer
    }

    pub fn initial() -> Self {
        let (player, opponent) = G::initial();
        Self::new(player, opponent)
    }

    pub fn to_string(&self) -> String {
        let mut ans: Vec<char> = Vec::with_capacity(G::CELL_COUNT);
        for y in 0..G::HEIGHT {
            for x in 0..G::WIDTH {
                let m = G::bit_at(x, y);
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
        let mut ans: Vec<char> = Vec::with_capacity(G::CELL_COUNT + G::HEIGHT);
        for y in 0..G::HEIGHT {
            for x in 0..G::WIDTH {
                let m = G::bit_at(x, y);
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

impl<G: Geometry> PartialEq for Board<G> {
    fn eq(&self, other: &Self) -> bool {
        self.player == other.player && self.opponent == other.opponent
    }
}

impl<G: Geometry> Eq for Board<G> {}

// OrdとPartialOrdを実装（C++のoperator <などに相当）
impl<G: Geometry> PartialOrd for Board<G> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<G: Geometry> Ord for Board<G> {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.player.cmp(&other.player) {
            Ordering::Equal => self.opponent.cmp(&other.opponent),
            ord => ord,
        }
    }
}

/// 1方向に対する「はさみ取り」判定。はさめるならその方向の反転集合を返す。
#[inline(always)]
fn ray_flips<G: Geometry>(move_bb: u64, player: u64, opponent: u64, dir: Direction) -> u64 {
    // 直後の1マスへ進める
    let mut x = G::shift(dir, move_bb);
    let mut flips = 0u64;

    // 連続する相手石を収集
    while x != 0 && (x & opponent) != 0 {
        flips |= x;
        x = G::shift(dir, x);
    }

    // その先に自石があるなら挟めている→反転成立
    if x & player != 0 {
        flips
    } else {
        0
    }
}

pub fn flip_generic<G: Geometry>(pos: usize, player: u64, opponent: u64) -> u64 {
    debug_assert!(pos < G::CELL_COUNT);
    let move_bb = G::bit_by_index(pos);

    // 盤上に既に石があるマスなら反転なし（安全のため）
    if (move_bb & (player | opponent)) != 0 {
        return 0;
    }

    Direction::ALL.iter().fold(0u64, |acc, dir| {
        acc | ray_flips::<G>(move_bb, player, opponent, *dir)
    })
}

pub fn get_moves_generic<G: Geometry>(player: u64, opponent: u64) -> u64 {
    let mut moves = 0u64;
    for pos in 0..G::CELL_COUNT {
        let bit = G::bit_by_index(pos);
        if bit & (player | opponent) == 0 && flip_generic::<G>(pos, player, opponent) != 0 {
            moves |= bit;
        }
    }
    moves
}

pub fn flip(pos: usize, player: u64, opponent: u64) -> u64 {
    flip_generic::<Standard8x8>(pos, player, opponent)
}

pub fn get_moves(player: u64, opponent: u64) -> u64 {
    get_moves_generic::<Standard8x8>(player, opponent)
}

#[derive(Debug, Clone, Copy)]
pub struct Standard6x6;

impl Standard6x6 {
    const REGION_MASK: u64 = 0x007E_7E7E_7E7E_7E00;

    #[inline(always)]
    const fn actual_index(x: usize, y: usize) -> usize {
        (y + 1) * 8 + (x + 1)
    }

    #[inline(always)]
    fn clamp(bb: u64) -> u64 {
        bb & Self::REGION_MASK
    }
}

impl Geometry for Standard6x6 {
    const WIDTH: usize = 6;
    const HEIGHT: usize = 6;

    fn initial() -> (u64, u64) {
        let player = Self::bit_at(2, 3) | Self::bit_at(3, 2);
        let opponent = Self::bit_at(2, 2) | Self::bit_at(3, 3);
        (player, opponent)
    }

    fn center_mask() -> u64 {
        Self::bit_at(2, 2) | Self::bit_at(3, 3) | Self::bit_at(2, 3) | Self::bit_at(3, 2)
    }

    fn region_mask() -> u64 {
        Self::REGION_MASK
    }

    fn bit_by_index(pos: usize) -> u64 {
        debug_assert!(pos < Self::CELL_COUNT);
        let x = pos % Self::WIDTH;
        let y = pos / Self::WIDTH;
        1u64 << Self::actual_index(x, y)
    }

    fn shift(dir: Direction, bb: u64) -> u64 {
        let bb = Self::clamp(bb);
        let region = Self::REGION_MASK;
        let shifted = match dir {
            Direction::East => bb << 1,
            Direction::SouthEast => bb >> 7,
            Direction::South => bb >> 8,
            Direction::SouthWest => bb >> 9,
            Direction::West => bb >> 1,
            Direction::NorthWest => bb << 7,
            Direction::North => bb << 8,
            Direction::NorthEast => bb << 9,
        };
        shifted & region
    }

    fn transpose(b: u64) -> u64 {
        Standard8x8::transpose(b) & Self::REGION_MASK
    }

    fn vertical_mirror(b: u64) -> u64 {
        Standard8x8::vertical_mirror(b) & Self::REGION_MASK
    }

    fn horizontal_mirror(b: u64) -> u64 {
        Standard8x8::horizontal_mirror(b) & Self::REGION_MASK
    }
}

pub type Board6 = Board<Standard6x6>;

pub fn flip6(pos: usize, player: u64, opponent: u64) -> u64 {
    flip_generic::<Standard6x6>(pos, player, opponent)
}

pub fn get_moves6(player: u64, opponent: u64) -> u64 {
    get_moves_generic::<Standard6x6>(player, opponent)
}
