use std::collections::HashSet;

use crate::othello::{board_with_symmetry, Board};

pub const FULL_ORBIT_SIZE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DistinguishedSample {
    pub original: Board,
    pub selected: Board,
    pub orbit_size: usize,
    pub selected_index: usize,
}

pub fn distinguished_variants(board: Board) -> Vec<Board> {
    let mut variants = Vec::with_capacity(FULL_ORBIT_SIZE);
    let mut seen = HashSet::with_capacity(FULL_ORBIT_SIZE);

    for sym in 0..8_i32 {
        let transformed = board_with_symmetry(board, sym);
        for candidate in [transformed, transformed.swapped()] {
            let key = board_key(candidate);
            if seen.insert(key) {
                variants.push(candidate);
            }
        }
    }

    variants.sort_unstable_by_key(|b| board_key(*b));
    variants
}

pub fn distinguished_orbit_key(board: Board) -> [u64; 2] {
    distinguished_variants(board)
        .into_iter()
        .map(board_key)
        .min()
        .expect("a board always has at least one distinguished variant")
}

pub fn deterministic_sample(board: Board, seed: u64) -> Option<DistinguishedSample> {
    let variants = distinguished_variants(board);
    let orbit_size = variants.len();
    debug_assert!(orbit_size > 0 && orbit_size <= FULL_ORBIT_SIZE);

    let mut rng = SplitMix64::new(seed ^ mix_board(board));
    let keep_draw = (rng.next_u64() % FULL_ORBIT_SIZE as u64) as usize;
    if keep_draw >= orbit_size {
        return None;
    }

    let selected_index = (rng.next_u64() % orbit_size as u64) as usize;
    Some(DistinguishedSample {
        original: board,
        selected: variants[selected_index],
        orbit_size,
        selected_index,
    })
}

pub fn board_key(board: Board) -> [u64; 2] {
    [board.player, board.opponent]
}

fn mix_board(board: Board) -> u64 {
    let mut x = board.player;
    x ^= board.opponent.rotate_left(17);
    splitmix64(x)
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[derive(Debug, Clone, Copy)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        splitmix64(self.state)
    }
}
