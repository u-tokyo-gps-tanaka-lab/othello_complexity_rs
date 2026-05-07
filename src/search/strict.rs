use std::collections::HashSet;

use crate::{
    othello::{get_moves, Board},
    prunings::{occupancy::check_occupancy, seg3::check_seg3_more},
    search::{
        core::{retrospective_flip, SearchResult},
        transposition::Btable,
    },
};

pub struct StrictLeafCache {
    searched: HashSet<[u64; 2]>,
    leaf: HashSet<[u64; 2]>,
}

impl StrictLeafCache {
    pub fn new(discs: i32) -> Self {
        let mut searched = HashSet::new();
        let mut leaf = HashSet::new();
        fwd_search_strict(&Board::initial(), &mut searched, &mut leaf, discs);
        Self { searched, leaf }
    }

    pub fn searched_count(&self) -> usize {
        self.searched.len()
    }

    pub fn leaf_count(&self) -> usize {
        self.leaf.len()
    }

    pub fn leaf(&self) -> &HashSet<[u64; 2]> {
        &self.leaf
    }
}

pub fn fwd_search_strict(
    board: &Board,
    searched: &mut HashSet<[u64; 2]>,
    leafnode: &mut HashSet<[u64; 2]>,
    discs: i32,
) {
    let key = [board.player, board.opponent];

    if board.popcount() >= discs as u32 {
        if get_moves(board.player, board.opponent) != 0 {
            leafnode.insert(key);
            return;
        } else if get_moves(board.opponent, board.player) != 0 {
            fwd_search_strict(&board.swapped(), searched, leafnode, discs);
        }
        return;
    }

    if !searched.insert(key) {
        return;
    }

    let mut moves = get_moves(board.player, board.opponent);
    if moves == 0 {
        if get_moves(board.opponent, board.player) != 0 {
            fwd_search_strict(&board.swapped(), searched, leafnode, discs);
        }
        return;
    }

    while moves != 0 {
        let idx = moves.trailing_zeros() as usize;
        moves &= moves - 1;

        let flipped = crate::othello::flip(idx, board.player, board.opponent);
        if flipped == 0 {
            continue;
        }
        let next = Board::new(
            board.opponent ^ flipped,
            board.player ^ (flipped | (1_u64 << idx)),
        );
        fwd_search_strict(&next, searched, leafnode, discs);
    }
}

pub fn retrospective_search_strict(
    board: &Board,
    from_pass: bool,
    discs: i32,
    leafnode: &HashSet<[u64; 2]>,
    retrospective_searched: &mut Btable,
    retroflips: &mut Vec<[u64; 10_000]>,
    node_count: &mut usize,
    node_limit: usize,
) -> SearchResult {
    let key = [board.player, board.opponent];
    let num_disc = board.popcount() as usize;

    if (num_disc as i32) <= discs {
        return if leafnode.contains(&key) {
            SearchResult::Found
        } else {
            SearchResult::NotFound
        };
    }

    if !retrospective_searched.insert(key) {
        return SearchResult::NotFound;
    }
    *node_count += 1;
    if *node_count > node_limit {
        return SearchResult::Unknown;
    }

    let occupied = board.player | board.opponent;
    if !check_occupancy(occupied) || !check_seg3_more(board.player, board.opponent, false) {
        return SearchResult::NotFound;
    }

    if !from_pass && get_moves(board.opponent, board.player) == 0 {
        let prev = board.swapped();
        match retrospective_search_strict(
            &prev,
            true,
            discs,
            leafnode,
            retrospective_searched,
            retroflips,
            node_count,
            node_limit,
        ) {
            SearchResult::Found => return SearchResult::Found,
            SearchResult::Unknown => return SearchResult::Unknown,
            SearchResult::NotFound => {}
        }
    }

    let mut b = board.opponent & !crate::othello::CENTER_MASK;
    if b == 0 {
        return SearchResult::NotFound;
    }

    if retroflips.len() <= num_disc {
        retroflips.resize(num_disc + 1, [0_u64; 10_000]);
    }

    while b != 0 {
        let index = b.trailing_zeros() as usize;
        b &= b - 1;

        let num = retrospective_flip(
            index,
            board.player,
            board.opponent,
            &mut retroflips[num_disc],
        );

        for i in 1..num {
            let flipped = retroflips[num_disc][i];
            debug_assert!(flipped != 0);

            let prev = Board::new(
                board.opponent ^ (flipped | (1_u64 << index)),
                board.player ^ flipped,
            );

            match retrospective_search_strict(
                &prev,
                false,
                discs,
                leafnode,
                retrospective_searched,
                retroflips,
                node_count,
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
