//! Oracles are the mixed-radix definition and finite enumeration of a 2×2 model.
#![allow(dead_code)]
mod support;

use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{Board, Color, Position, CENTER_MASK};
use othello_complexity_rs::sampling::{
    create_output_dir, fixed_discs_class_population, fixed_discs_population, is_representative,
    position_from_rank, rank_of_position, read_sample_dir, representative_candidates,
    sample_fixed_discs, select_representative, Population, CLASS_POPULATION_SIZE, POPULATION_SIZE,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use support::ArrayPosition;

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

fn geometric_class_key(board: Board) -> (u64, u64) {
    ArrayPosition::from_relative(&board.to_string(), true)
        .orbit()
        .map(|image| {
            let board = image.position().board;
            (board.player | board.opponent, board.player)
        })
        .min()
        .unwrap()
}

fn central_coloring(colors: u32) -> Board {
    let mut player = 0;
    for (index, square) in [27, 28, 35, 36].into_iter().enumerate() {
        if colors & (1 << index) != 0 {
            player |= 1 << square;
        }
    }
    Board::new(player, CENTER_MASK & !player)
}

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
fn fixed_disc_sampling_has_its_own_population_and_keeps_both_turns() {
    assert_eq!(fixed_discs_population(4).unwrap(), 32);
    assert_eq!(fixed_discs_population(5).unwrap(), 60 * 64);
    assert_eq!(fixed_discs_population(64).unwrap(), 1_u128 << 65);
    assert!(fixed_discs_population(3).is_err());
    assert!(fixed_discs_population(65).is_err());
    let mut rng = ChaCha8Rng::seed_from_u64(41);
    let mut observed = HashSet::new();
    for discs in 4..=64 {
        for _ in 0..20 {
            let position = sample_fixed_discs(&mut rng, discs).unwrap();
            let occupied = position.board.player | position.board.opponent;
            assert_eq!(occupied.count_ones(), discs);
            assert_eq!(occupied & CENTER_MASK, CENTER_MASK);
            observed.insert(position.side_to_move);
        }
    }
    assert_eq!(observed.len(), 2);
}

#[test]
fn class_population_matches_independent_small_board_enumeration() {
    // Enumerate all 16 central colorings, and then all 60 × 2 single outer
    // discs. Coordinate-based transformations, not the Burnside implementation,
    // decide whether two boards belong to the same class.
    let mut four_disc_classes = HashSet::new();
    let mut five_disc_classes = HashSet::new();
    for colors in 0..16 {
        let board = central_coloring(colors);
        four_disc_classes.insert(geometric_class_key(board));
        for square in 0..64 {
            let bit = 1 << square;
            if bit & CENTER_MASK != 0 {
                continue;
            }
            for outer in [
                Board::new(board.player | bit, board.opponent),
                Board::new(board.player, board.opponent | bit),
            ] {
                five_disc_classes.insert(geometric_class_key(outer));
            }
        }
    }
    assert_eq!(four_disc_classes.len(), 6);
    assert_eq!(five_disc_classes.len(), 264);
    assert_eq!(
        fixed_discs_class_population(4).unwrap(),
        four_disc_classes.len() as u128
    );
    assert_eq!(
        fixed_discs_class_population(5).unwrap(),
        five_disc_classes.len() as u128
    );
    assert!(fixed_discs_class_population(3).is_err());
    assert!(fixed_discs_class_population(65).is_err());
}

#[test]
fn population_sizes_include_self_symmetric_classes_and_partition_by_disc_count() {
    // Burnside cycle counts for a full binary 8×8 board are 64, 16, 32,
    // 16, 32, 32, 36, 36; fixed points give the exact boundary value below.
    assert_eq!(
        fixed_discs_class_population(64).unwrap(),
        2_305_843_028_004_192_256
    );
    assert_eq!(
        CLASS_POPULATION_SIZE,
        84_782_316_550_443_833_986_427_293_875
    );
    assert!(CLASS_POPULATION_SIZE > POPULATION_SIZE / 16);
    for population in [Population::Absolute, Population::Classes] {
        let sum: u128 = (4..=64)
            .map(|discs| population.size(Some(discs)).unwrap())
            .sum();
        assert_eq!(population.size(None).unwrap(), sum);
    }
}

#[test]
fn population_metadata_identifies_the_sampled_set_exactly() {
    for (population, discs, definition, size) in [
        (
            Population::Absolute,
            None,
            "absolute_positions_center_occupied",
            POPULATION_SIZE,
        ),
        (
            Population::Absolute,
            Some(4),
            "absolute_positions_fixed_discs",
            32,
        ),
        (
            Population::Classes,
            None,
            "equivalence_classes_center_occupied",
            CLASS_POPULATION_SIZE,
        ),
        (
            Population::Classes,
            Some(4),
            "equivalence_classes_fixed_discs",
            6,
        ),
    ] {
        let mut expected = serde_json::json!({"definition": definition, "size": size.to_string()});
        if let Some(discs) = discs {
            expected["discs"] = serde_json::json!(discs);
        }
        assert_eq!(population.metadata(discs).unwrap(), expected);
    }
}

#[test]
fn representative_rejection_gives_equal_mass_to_self_symmetric_classes() {
    // Every equally likely four-disc proposal is enumerated once. Exactly one
    // proposal per class survives, even though their orbit sizes differ.
    let mut orbit_sizes = HashMap::new();
    let mut accepted = HashMap::new();
    for colors in 0..16 {
        let board = central_coloring(colors);
        let key = geometric_class_key(board);
        *orbit_sizes.entry(key).or_insert(0) += 1;
        if is_representative(board) {
            *accepted.entry(key).or_insert(0) += 1;
        }
    }
    assert_eq!(
        orbit_sizes.values().copied().collect::<HashSet<_>>(),
        HashSet::from([1, 2, 4])
    );
    assert_eq!(accepted.len(), 6);
    assert!(accepted.values().all(|&count| count == 1));

    // Exercise the actual sampler as well: canonicalizing every proposal would
    // give counts near 375, 750 or 1500 rather than 1000 for each class.
    let mut rng = ChaCha8Rng::seed_from_u64(20261022);
    let mut observed = HashMap::new();
    for _ in 0..6000 {
        let position = Population::Classes.sample(&mut rng, Some(4)).unwrap();
        assert_eq!(position.side_to_move, Color::Black);
        assert_eq!(position.board.player | position.board.opponent, CENTER_MASK);
        assert_eq!(geometric_class_key(position.board).1, position.board.player);
        *observed.entry(position).or_insert(0) += 1;
    }
    assert_eq!(observed.len(), 6);
    assert!(observed.values().all(|count| (750..=1250).contains(count)));
}

#[test]
fn selected_population_sampling_preserves_disc_count_and_reproducibility() {
    for population in [Population::Absolute, Population::Classes] {
        let mut first = ChaCha8Rng::seed_from_u64(53);
        let mut second = ChaCha8Rng::seed_from_u64(53);
        let mut turns = HashSet::new();
        for discs in [None, Some(4), Some(5), Some(32), Some(64)] {
            for _ in 0..32 {
                let position = population.sample(&mut first, discs).unwrap();
                assert_eq!(position, population.sample(&mut second, discs).unwrap());
                turns.insert(position.side_to_move);
                let occupied = position.board.player | position.board.opponent;
                assert_eq!(occupied & CENTER_MASK, CENTER_MASK);
                if let Some(discs) = discs {
                    assert_eq!(occupied.count_ones(), discs);
                }
                if population == Population::Classes {
                    assert_eq!(position.side_to_move, Color::Black);
                    assert_eq!(
                        geometric_class_key(position.board),
                        (occupied, position.board.player)
                    );
                }
            }
        }
        assert_eq!(
            turns.len(),
            if population == Population::Absolute {
                2
            } else {
                1
            }
        );
        assert!(population.sample(&mut first, Some(3)).is_err());
        assert!(population.sample(&mut first, Some(65)).is_err());
    }
}

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

#[test]
fn create_output_dir_removes_sampling_record_and_preserves_other_files() {
    let directory = std::env::temp_dir().join(format!(
        "othello-sampling-output-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let positions = format!("{}\n", format_position(Position::initial()));
    fs::write(directory.join("positions.txt"), &positions).unwrap();
    fs::write(directory.join("keep.txt"), b"keep").unwrap();
    fs::write(
        directory.join("sampling.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "method": "fixed-discs",
            "population": Population::Absolute.metadata(Some(4)).unwrap(),
            "uniform": true,
            "with_replacement": true,
            "seed": 1,
            "accepted_count": 1,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(read_sample_dir(&directory).is_ok());

    create_output_dir(&directory, "sampling.json").unwrap();

    assert!(!directory.join("sampling.json").exists());
    assert_eq!(
        fs::read_to_string(directory.join("positions.txt")).unwrap(),
        positions
    );
    assert_eq!(fs::read(directory.join("keep.txt")).unwrap(), b"keep");
    assert!(read_sample_dir(&directory).is_err());
    fs::remove_dir_all(directory).unwrap();
}
