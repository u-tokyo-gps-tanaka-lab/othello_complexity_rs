//! The mixed-radix population definition is the decoding oracle.
#[allow(dead_code)]
mod support;

use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{Board, Color, Position, CENTER_MASK};
use othello_complexity_rs::sampling::{position_from_rank, rank_of_position, POPULATION_SIZE};

#[test]
fn ranks_use_the_declared_low_turn_bit_and_cell_radices() {
    let first = Position {
        board: Board::new(CENTER_MASK, 0),
        side_to_move: Color::Black,
    };
    assert_eq!(position_from_rank(0).unwrap(), first);
    assert_eq!(
        position_from_rank(1).unwrap(),
        Position {
            side_to_move: Color::White,
            ..first
        }
    );
    for (rank, player, opponent) in [
        (2, CENTER_MASK | 1, 0),
        (4, CENTER_MASK, 1),
        (6, CENTER_MASK | 2, 0),
        (2 * 3_u128.pow(27), CENTER_MASK & !(1 << 27), 1 << 27),
        (4 * 3_u128.pow(27), CENTER_MASK & !(1 << 28), 1 << 28),
        (8 * 3_u128.pow(33), CENTER_MASK & !(1 << 35), 1 << 35),
        (16 * 3_u128.pow(33), CENTER_MASK & !(1 << 36), 1 << 36),
        (32 * 3_u128.pow(33), CENTER_MASK | (1 << 37), 0),
    ] {
        assert_eq!(
            position_from_rank(rank).unwrap(),
            Position {
                board: Board::new(player, opponent),
                side_to_move: Color::Black
            }
        );
    }
    assert_eq!(POPULATION_SIZE, 1_356_517_064_806_918_512_457_421_862_432);
    assert_eq!(
        position_from_rank(POPULATION_SIZE - 1).unwrap(),
        Position {
            board: Board::new(0, u64::MAX),
            side_to_move: Color::White
        }
    );
    for rank in (0..1000).chain([
        POPULATION_SIZE / 2,
        POPULATION_SIZE - 2,
        POPULATION_SIZE - 1,
    ]) {
        assert_eq!(
            rank_of_position(position_from_rank(rank).unwrap()).unwrap(),
            rank
        );
    }
    assert!(position_from_rank(POPULATION_SIZE).is_err());
    assert!(position_from_rank(u128::MAX).is_err());
    assert!(rank_of_position(Position {
        board: Board::empty(),
        ..first
    })
    .is_err());
}

#[test]
fn changing_the_suffix_exchanges_absolute_colors_together_with_the_turn() {
    let black = Position::initial();
    let color_exchanged = Position {
        side_to_move: Color::White,
        ..black
    };
    assert_eq!(
        rank_of_position(color_exchanged).unwrap(),
        rank_of_position(black).unwrap() + 1
    );
    assert_eq!(
        &format_position(black)[..64],
        &format_position(color_exchanged)[..64]
    );
    let turn_only = Position {
        board: black.board.swapped(),
        side_to_move: Color::White,
    };
    assert_ne!(
        rank_of_position(turn_only).unwrap(),
        rank_of_position(color_exchanged).unwrap()
    );
}

// Append to tests/sampling_test.rs; its crate root must declare mod support once.
mod conversion_contracts {
    use super::support::ArrayPosition;
    use othello_complexity_rs::othello::{Board, Position, CENTER_MASK};
    use othello_complexity_rs::sampling::{
        is_representative, representative_candidates, select_representative,
    };
    use std::collections::{HashMap, HashSet};

    #[test]
    fn each_orbit_size_has_exactly_m_accepted_draws_out_of_sixteen() {
        for (board, expected) in [
            (Board::new(CENTER_MASK, 0), 2),
            (Board::initial(), 4),
            (Board::new(CENTER_MASK | 1, 0), 8),
            (Board::new(CENTER_MASK | (1 << 8), 0), 16),
        ] {
            let candidates = representative_candidates(board);
            assert_eq!(candidates.len(), expected);
            let oracle = ArrayPosition::from_relative(&board.to_string(), true);
            let expected_positions: HashSet<_> = oracle
                .orbit()
                .flat_map(|p| [p, p.color_exchanged()])
                .map(|p| p.position())
                .collect();
            assert_eq!(
                candidates.iter().map(|(_, p)| *p).collect::<HashSet<_>>(),
                expected_positions
            );
            let accepted: Vec<_> = (0..16)
                .filter_map(|draw| select_representative(&candidates, draw).unwrap())
                .collect();
            assert_eq!(accepted.len(), expected);
            assert_eq!(
                accepted
                    .iter()
                    .map(|(_, p)| p)
                    .collect::<HashSet<_>>()
                    .len(),
                expected
            );
        }
    }

    #[test]
    fn exhaustive_two_by_two_model_gives_each_absolute_candidate_the_same_rational_mass() {
        // The four corners form a 2×2 ternary model; the central all-X block is fixed
        // by every square symmetry. There are 3^4 boards, and 2 absolute turns.
        let mut masses: HashMap<Position, usize> = HashMap::new();
        let mut trials = 0;
        let mut rejects = 0;
        let mut representative_count = 0;
        for mut rank in 0..81 {
            let mut board = Board::new(CENTER_MASK, 0);
            for square in [0, 7, 56, 63] {
                match rank % 3 {
                    1 => board.player |= 1 << square,
                    2 => board.opponent |= 1 << square,
                    _ => (),
                }
                rank /= 3;
            }
            let oracle = ArrayPosition::from_relative(&board.to_string(), true);
            let minimum = oracle
                .orbit()
                .map(|p| {
                    let b = p.position().board;
                    (b.player | b.opponent, b.player)
                })
                .min()
                .unwrap();
            let retained = (board.player | board.opponent, board.player) == minimum;
            assert_eq!(is_representative(board), retained);
            if !retained {
                continue;
            }
            representative_count += 1;
            let candidates = representative_candidates(board);
            for draw in 0..16 {
                trials += 1;
                if let Some((_, position)) = select_representative(&candidates, draw).unwrap() {
                    *masses.entry(position).or_default() += 1;
                } else {
                    rejects += 1;
                }
            }
        }
        // Each original representative has weight 1/r; each draw weight 1/16.
        // Integer numerator 1 for every candidate proves equality without a
        // frequency tolerance. Conditional on acceptance, the mass is 1/(2*81).
        assert_eq!(trials, 16 * representative_count);
        assert_eq!(masses.len(), 2 * 81);
        assert!(masses.values().all(|&numerator| numerator == 1));
        assert_eq!(trials - rejects, 162);
        assert!(rejects > 0);
    }
}
