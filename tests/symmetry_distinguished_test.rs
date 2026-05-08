use std::collections::HashSet;

use othello_complexity_rs::{
    io::parse_line_to_board,
    othello::Board,
    search::{
        core::SearchResult, parallel_gbfs::parallel_retrospective_greedy_best_first_search_strict,
        strict::strict_fwd_search,
    },
    symmetry_distinguished::{
        board_key, deterministic_sample, distinguished_orbit_key, distinguished_variants,
        FULL_ORBIT_SIZE,
    },
};

#[test]
fn full_orbit_board_has_sixteen_distinguished_variants() {
    let board =
        parse_line_to_board("OX-OO-XX----O--OXXX-O-X-OXXOOO-XO-XOOXOOXX--X-XOXOOOX-OOOXOX-O-X")
            .unwrap();
    let variants = distinguished_variants(board);
    let unique = variants
        .iter()
        .map(|b| board_key(*b))
        .collect::<HashSet<_>>();

    assert_eq!(variants.len(), FULL_ORBIT_SIZE);
    assert_eq!(unique.len(), variants.len());
}

#[test]
fn self_symmetric_board_has_smaller_orbit() {
    let board = Board::new(u64::MAX, 0);
    let variants = distinguished_variants(board);

    assert_eq!(variants.len(), 2);
    assert_eq!(distinguished_orbit_key(board), [0, u64::MAX]);
}

#[test]
fn deterministic_sampling_is_reproducible() {
    let board =
        parse_line_to_board("OX-OO-XX----O--OXXX-O-X-OXXOOO-XO-XOOXOOXX--X-XOXOOOX-OOOXOX-O-X")
            .unwrap();
    let first = deterministic_sample(board, 123).unwrap();
    let second = deterministic_sample(board, 123).unwrap();

    assert_eq!(first.selected, second.selected);
    assert_eq!(first.orbit_size, FULL_ORBIT_SIZE);
}

#[test]
fn strict_parallel_gbfs_finds_the_initial_position_without_symmetry() {
    let mut searched = HashSet::new();
    let mut leaf = HashSet::new();
    strict_fwd_search(&Board::initial(), &mut searched, &mut leaf, 4);
    let mut leaf_vec = leaf.iter().copied().collect::<Vec<_>>();
    leaf_vec.sort_unstable();

    let result = parallel_retrospective_greedy_best_first_search_strict(
        &Board::initial(),
        4,
        &leaf_vec,
        100,
        false,
        2,
    );

    assert_eq!(result, SearchResult::Found);
}
