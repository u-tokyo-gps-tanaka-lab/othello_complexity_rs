//! Oracles are the mixed-radix definition and finite enumeration of a 2×2 model.
#![allow(dead_code)]
mod support;

use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{Board, Color, Position, CENTER_MASK};
use othello_complexity_rs::sampling::{
    artifact, class_representative, convert_classes, create_output_dir, ensure_distinct_files,
    fixed_discs_population, generate, is_representative, position_from_rank, rank_of_position,
    read_sample_dir, representative_candidates, sample_fixed_discs, select_representative, sha256,
    Population, POPULATION_SIZE,
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
fn population_sizes_count_black_to_move_positions_and_partition_by_disc_count() {
    assert_eq!(
        Population::Classes.size(None).unwrap(),
        2_u128.pow(4) * 3_u128.pow(60)
    );
    assert_eq!(Population::Classes.size(Some(4)).unwrap(), 16);
    assert_eq!(Population::Classes.size(Some(5)).unwrap(), 1_920);
    assert_eq!(Population::Classes.size(Some(64)).unwrap(), 2_u128.pow(64));
    for population in [Population::Absolute, Population::Classes] {
        let sum: u128 = (4..=64)
            .map(|discs| population.size(Some(discs)).unwrap())
            .sum();
        assert_eq!(population.size(None).unwrap(), sum);
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

    // Each of the 16 central colorings is sampled independently. Six are
    // representatives, so 16,000 attempts yield 6,000 accepted positions in
    // expectation and 1,000 occurrences of each representative in expectation.
    let mut rng = ChaCha8Rng::seed_from_u64(20261022);
    let mut observed = HashMap::new();
    let mut accepted_total = 0;
    for _ in 0..16_000 {
        let Some(position) = Population::Classes.sample(&mut rng, Some(4)).unwrap() else {
            continue;
        };
        accepted_total += 1;
        assert_eq!(position.side_to_move, Color::Black);
        assert_eq!(position.board.player | position.board.opponent, CENTER_MASK);
        assert_eq!(geometric_class_key(position.board).1, position.board.player);
        *observed.entry(position).or_insert(0) += 1;
    }
    assert_eq!(observed.len(), 6);
    assert!(observed.values().all(|count| (750..=1250).contains(count)));
    assert!((5_700..=6_300).contains(&accepted_total));
}

#[test]
fn fixed_disc_and_selected_population_sampling_preserve_counts_turns_and_reproducibility() {
    assert_eq!(fixed_discs_population(4).unwrap(), 32);
    assert_eq!(fixed_discs_population(5).unwrap(), 60 * 64);
    assert_eq!(fixed_discs_population(64).unwrap(), 1_u128 << 65);
    assert!(fixed_discs_population(3).is_err());
    assert!(fixed_discs_population(65).is_err());
    let mut fixed_rng = ChaCha8Rng::seed_from_u64(41);
    for discs in 4..=64 {
        let mut turns = HashSet::new();
        for _ in 0..20 {
            let position = sample_fixed_discs(&mut fixed_rng, discs).unwrap();
            let occupied = position.board.player | position.board.opponent;
            assert_eq!(occupied.count_ones(), discs, "{discs} discs");
            assert_eq!(occupied & CENTER_MASK, CENTER_MASK, "{discs} discs");
            turns.insert(position.side_to_move);
        }
        assert_eq!(turns.len(), 2, "{discs} discs");
    }

    for (case, population, expected_turns) in [
        ("absolute positions", Population::Absolute, 2),
        ("equivalence classes", Population::Classes, 1),
    ] {
        let mut first = ChaCha8Rng::seed_from_u64(53);
        let mut second = ChaCha8Rng::seed_from_u64(53);
        for discs in [None, Some(4), Some(5), Some(32), Some(64)] {
            let mut turns = HashSet::new();
            for _ in 0..32 {
                let sampled = population.sample(&mut first, discs).unwrap();
                assert_eq!(
                    sampled,
                    population.sample(&mut second, discs).unwrap(),
                    "{case}, {discs:?} discs"
                );
                let Some(position) = sampled else {
                    continue;
                };
                turns.insert(position.side_to_move);
                let occupied = position.board.player | position.board.opponent;
                assert_eq!(
                    occupied & CENTER_MASK,
                    CENTER_MASK,
                    "{case}, {discs:?} discs"
                );
                if let Some(discs) = discs {
                    assert_eq!(occupied.count_ones(), discs, "{case}, {discs} discs");
                }
                if population == Population::Classes {
                    assert_eq!(
                        position.side_to_move,
                        Color::Black,
                        "{case}, {discs:?} discs"
                    );
                    assert_eq!(
                        geometric_class_key(position.board),
                        (occupied, position.board.player),
                        "{case}, {discs:?} discs"
                    );
                }
            }
            assert_eq!(turns.len(), expected_turns, "{case}, {discs:?} discs");
        }
        assert!(population.sample(&mut first, Some(3)).is_err(), "{case}");
        assert!(population.sample(&mut first, Some(65)).is_err(), "{case}");
    }
}

#[test]
fn representative_candidates_match_the_oracle_and_have_equal_rational_mass() {
    for (case, board, expected) in [
        ("fully symmetric", Board::new(CENTER_MASK, 0), 2),
        ("initial board", Board::initial(), 4),
        ("corner extension", Board::new(CENTER_MASK | 1, 0), 8),
        ("edge extension", Board::new(CENTER_MASK | (1 << 8), 0), 16),
    ] {
        let candidates = representative_candidates(board);
        assert_eq!(candidates.len(), expected, "{case}");
        let oracle = ArrayPosition::from_relative(&board.to_string(), true);
        let expected_positions: HashSet<_> = oracle
            .orbit()
            .flat_map(|p| [p, p.color_exchanged()])
            .map(|p| p.position())
            .collect();
        assert_eq!(
            candidates.iter().map(|(_, p)| *p).collect::<HashSet<_>>(),
            expected_positions,
            "{case}"
        );
        let accepted: Vec<_> = (0..16)
            .filter_map(|selection_index| {
                select_representative(&candidates, selection_index).unwrap()
            })
            .collect();
        assert_eq!(accepted.len(), expected, "{case}");
        assert_eq!(
            accepted
                .iter()
                .map(|(_, p)| p)
                .collect::<HashSet<_>>()
                .len(),
            expected,
            "{case}"
        );
    }

    // The four corners form a 2×2 ternary model; the central all-X block is fixed
    // by every square symmetry. There are 3^4 boards, and 2 absolute turns.
    let mut masses: HashMap<Position, usize> = HashMap::new();
    let mut trials = 0;
    let mut rejects = 0;
    let mut representative_count = 0;
    for encoded_rank in 0..81 {
        let mut rank = encoded_rank;
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
        let expected_positions: HashSet<_> = ArrayPosition::from_relative(&board.to_string(), true)
            .orbit()
            .flat_map(|position| [position, position.color_exchanged()])
            .map(|position| position.position())
            .collect();
        assert_eq!(
            candidates
                .iter()
                .map(|(_, position)| *position)
                .collect::<HashSet<_>>(),
            expected_positions,
            "rank {encoded_rank}"
        );
        for selection_index in 0..16 {
            trials += 1;
            if let Some((_, position)) =
                select_representative(&candidates, selection_index).unwrap()
            {
                *masses.entry(position).or_default() += 1;
            } else {
                rejects += 1;
            }
        }
    }
    // Each original representative has weight 1/r; each selection has weight 1/16.
    // Integer numerator 1 for every candidate proves equality without a
    // frequency tolerance. Conditional on acceptance, the mass is 1/(2*81).
    assert_eq!(trials, 16 * representative_count);
    assert_eq!(masses.len(), 2 * 81);
    assert!(masses.values().all(|&numerator| numerator == 1));
    assert_eq!(trials - rejects, 162);
    assert!(rejects > 0);
}

#[test]
fn invalid_generation_settings_do_not_create_an_output_directory() {
    for (label, discs, count) in [("count", Some(4), 0), ("discs", Some(3), 1)] {
        let directory = std::env::temp_dir().join(format!(
            "othello-invalid-generation-{label}-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let error = generate(&directory, Population::Absolute, discs, count, 17).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!directory.exists());
    }
}

#[test]
fn generated_samples_are_reproducible_and_pass_sample_validation() {
    let first = std::env::temp_dir().join(format!(
        "othello-generated-sample-first-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let second = std::env::temp_dir().join(format!(
        "othello-generated-sample-second-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));

    generate(&first, Population::Absolute, Some(8), 16, 20261031).unwrap();
    generate(&second, Population::Absolute, Some(8), 16, 20261031).unwrap();

    assert_eq!(
        fs::read(first.join("positions.txt")).unwrap(),
        fs::read(second.join("positions.txt")).unwrap()
    );
    for directory in [&first, &second] {
        let sample = read_sample_dir(directory).unwrap();
        assert_eq!(sample.population, Population::Absolute);
        assert_eq!(sample.discs, Some(8));
        assert_eq!(sample.positions.len(), 16);
        assert_eq!(sample.sample_size, 16);
    }

    fs::remove_dir_all(first).unwrap();
    fs::remove_dir_all(second).unwrap();
}

#[test]
fn equivalence_generation_records_sample_size_and_only_writes_representatives() {
    let directory = std::env::temp_dir().join(format!(
        "othello-equivalence-generation-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let sample_size = 64;
    let seed = 20261107;
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let expected = (1..=sample_size)
        .filter_map(|input_line| {
            Population::Classes
                .sample(&mut rng, Some(4))
                .unwrap()
                .map(|position| (input_line, position))
        })
        .collect::<Vec<_>>();
    assert!(!expected.is_empty());
    assert!(expected.len() < sample_size);

    generate(&directory, Population::Classes, Some(4), sample_size, seed).unwrap();

    let sample = read_sample_dir(&directory).unwrap();
    assert_eq!(sample.sample_size, sample_size);
    assert_eq!(
        sample.positions,
        expected
            .iter()
            .map(|(_, position)| *position)
            .collect::<Vec<_>>()
    );
    assert!(sample.positions.iter().all(|position| {
        position.side_to_move == Color::Black && is_representative(position.board)
    }));
    assert_eq!(sample.metadata["input_count"], sample_size);
    assert_eq!(sample.metadata["accepted_count"], expected.len());

    let mapping = fs::read_to_string(directory.join("mapping.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(mapping.len(), expected.len());
    for (output_index, (row, (input_line, position))) in mapping.iter().zip(&expected).enumerate() {
        assert_eq!(row["input_line"], *input_line);
        assert_eq!(row["output_line"], output_index + 1);
        assert_eq!(row["accepted"], true);
        assert_eq!(row["position"], format_position(*position));
    }
    assert!(mapping
        .iter()
        .enumerate()
        .any(|(index, row)| row["input_line"] != index + 1));

    fs::remove_dir_all(directory).unwrap();
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

#[test]
fn class_samples_convert_to_valid_uniform_absolute_samples() {
    let root = std::env::temp_dir().join(format!(
        "othello-class-conversion-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    let unrestricted_source = root.join("unrestricted-source");
    let fixed_source = root.join("fixed-source");
    let unrestricted_output = root.join("unrestricted-output");
    let fixed_output = root.join("fixed-output");
    generate(&unrestricted_source, Population::Classes, None, 256, 101).unwrap();
    generate(&fixed_source, Population::Classes, Some(5), 1_920, 102).unwrap();
    convert_classes(&unrestricted_source, &unrestricted_output, 201).unwrap();
    convert_classes(&fixed_source, &fixed_output, 202).unwrap();

    let unrestricted = read_sample_dir(&unrestricted_output).unwrap();
    assert_eq!(unrestricted.population, Population::Absolute);
    assert_eq!(unrestricted.discs, None);
    let fixed = read_sample_dir(&fixed_output).unwrap();
    assert_eq!(fixed.population, Population::Absolute);
    assert_eq!(fixed.discs, Some(5));

    let verify = |source: &std::path::Path, output: &std::path::Path| {
        let source_sample = read_sample_dir(source).unwrap();
        let converted_sample = read_sample_dir(output).unwrap();
        let rows = fs::read_to_string(output.join("mapping.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), source_sample.positions.len());
        let mut accepted = Vec::new();
        let mut rejected = 0;
        for (index, (&source_position, row)) in
            source_sample.positions.iter().zip(&rows).enumerate()
        {
            let candidates = representative_candidates(source_position.board);
            let selection_index = row["selection_index"].as_u64().unwrap() as usize;
            let selected = candidates.get(selection_index).copied();
            assert_eq!(row["input_line"], index + 1);
            assert_eq!(row["source_position"], format_position(source_position));
            assert_eq!(row["candidate_count"], candidates.len());
            assert_eq!(row["accepted"], selected.is_some());
            assert_eq!(selection_index < candidates.len(), selected.is_some());
            if let Some((symmetry, position)) = selected {
                accepted.push(position);
                assert_eq!(row["output_line"], accepted.len());
                assert_eq!(row["symmetry"], symmetry.as_str());
                assert_eq!(
                    row["side_to_move"],
                    if position.side_to_move == Color::Black {
                        "b"
                    } else {
                        "w"
                    }
                );
                assert_eq!(row["position"], format_position(position));
                assert_eq!(class_representative(position), source_position);
            } else {
                rejected += 1;
                assert!(row["output_line"].is_null());
                assert!(row["symmetry"].is_null());
                assert!(row["side_to_move"].is_null());
                assert!(row["position"].is_null());
            }
        }
        assert_eq!(converted_sample.positions, accepted);
        let source_bytes = fs::read(source.join("positions.txt")).unwrap();
        assert_eq!(
            converted_sample.metadata["input"]["sha256"],
            sha256(&source_bytes)
        );
        assert_eq!(
            converted_sample.metadata["input"],
            artifact(&source.join("positions.txt")).unwrap()
        );
        assert_eq!(
            converted_sample.metadata["source_sampling"],
            artifact(&source.join("sampling.json")).unwrap()
        );
        (converted_sample.positions.len(), rejected)
    };

    verify(&unrestricted_source, &unrestricted_output);
    let (accepted, rejected) = verify(&fixed_source, &fixed_output);
    assert!(accepted > 0);
    assert!(rejected > 0);

    let exact_error_output = root.join("exact-error");
    assert!(convert_classes(&fixed_output, &exact_error_output, 302).is_err());
    assert!(!exact_error_output.exists());
    let repeated_seed_output = root.join("repeated-seed-error");
    assert!(convert_classes(&fixed_source, &repeated_seed_output, 102).is_err());
    assert!(!repeated_seed_output.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_equivalence_sample_cannot_be_converted() {
    let root = std::env::temp_dir().join(format!(
        "othello-empty-equivalence-conversion-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let source = root.join("source");
    let output = root.join("output");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("positions.txt"), b"").unwrap();
    fs::write(source.join("mapping.jsonl"), b"").unwrap();
    fs::write(
        source.join("sampling.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "method": "fixed-discs",
            "population": Population::Classes.metadata(Some(4)).unwrap(),
            "uniform": true,
            "with_replacement": true,
            "seed": 1,
            "input_count": 8,
            "accepted_count": 0,
        }))
        .unwrap(),
    )
    .unwrap();
    let sample = read_sample_dir(&source).unwrap();
    assert_eq!(sample.sample_size, 8);
    assert!(sample.positions.is_empty());

    assert!(convert_classes(&source, &output, 2).is_err());
    assert!(!output.join("sampling.json").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn input_cannot_alias_an_output_through_a_path_or_hard_link() {
    let directory = std::env::temp_dir().join(format!(
        "othello-same-conversion-file-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let input = directory.join("input.txt");
    let hard_link = directory.join("hard-link.txt");
    let distinct = directory.join("distinct.txt");
    let not_yet_existing = directory.join("not-yet-existing.txt");
    fs::write(&input, Board::initial().to_string()).unwrap();
    fs::hard_link(&input, &hard_link).unwrap();
    fs::write(&distinct, Board::initial().to_string()).unwrap();

    for output in [&input, &hard_link] {
        let error = ensure_distinct_files(&input, &[output]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
    ensure_distinct_files(&input, &[&distinct]).unwrap();
    ensure_distinct_files(&input, &[&not_yet_existing]).unwrap();
    fs::remove_dir_all(directory).unwrap();
}
