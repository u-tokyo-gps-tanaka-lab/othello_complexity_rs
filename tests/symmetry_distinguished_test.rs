use std::collections::HashSet;

use othello_complexity_rs::{
    io::parse_line_to_board,
    othello::{board_with_symmetry, Board},
    search::{
        core::SearchResult,
        dfs::retrospective_search,
        strict::{fwd_search_strict, retrospective_search_strict, StrictLeafCache},
        transposition::{Btable, LeafCache},
    },
};

fn symmetry_turn_orbit(board: Board) -> Vec<Board> {
    let mut variants = Vec::new();
    for turn_swap in [false, true] {
        let base = if turn_swap { board.swapped() } else { board };
        for sym in 0..8_i32 {
            variants.push(board_with_symmetry(base, sym));
        }
    }
    variants.sort();
    variants.dedup();
    variants
}

#[test]
fn strict_forward_keeps_opening_symmetries_distinct() {
    let mut searched = HashSet::new();
    let mut leaf = HashSet::new();
    fwd_search_strict(&Board::initial(), &mut searched, &mut leaf, 5);

    assert_eq!(leaf.len(), 4);
}

#[test]
fn strict_search_rejects_a_symmetric_leaf_that_quotient_search_accepts() {
    let unreachable_orientation =
        parse_line_to_board("---------------------------OOO-----XO---------------------------")
            .unwrap();

    let quotient_leaf = LeafCache::new(5);
    let strict_leaf = StrictLeafCache::new(5);
    let mut quotient_seen = Btable::new(128, 16);
    let mut strict_seen = Btable::new(128, 16);
    let mut retro = Vec::new();
    let mut nodes = 0;

    assert_eq!(
        retrospective_search(
            &unreachable_orientation,
            false,
            5,
            quotient_leaf.leaf(),
            &mut quotient_seen,
            &mut retro,
            &mut nodes,
            1000,
        ),
        SearchResult::Found
    );

    retro.clear();
    nodes = 0;
    assert_eq!(
        retrospective_search_strict(
            &unreachable_orientation,
            false,
            5,
            strict_leaf.leaf(),
            &mut strict_seen,
            &mut retro,
            &mut nodes,
            1000,
        ),
        SearchResult::NotFound
    );
}

#[test]
fn symmetry_turn_orbit_has_at_most_sixteen_states() {
    let board = Board::new(0x0000_0008_3820_0000, 0x0000_0010_0400_0000);
    let orbit = symmetry_turn_orbit(board);

    assert!(!orbit.is_empty());
    assert!(orbit.len() <= 16);
}
