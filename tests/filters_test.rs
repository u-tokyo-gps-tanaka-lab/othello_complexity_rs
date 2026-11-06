//! Necessary conditions from the absolute-position pipeline contract:
//! independent legal play must pass, and colour/turn/geometric changes must
//! preserve these deliberately relaxed conditions.

#[allow(dead_code)]
mod support;

use std::collections::{HashSet, VecDeque};

use othello_complexity_rs::filters::{
    impossible_edges::{check_edge_patterns, check_virtual_edge_patterns},
    occupancy::check_occupancy,
    seg3::check_seg3_more,
    Filter,
};
use othello_complexity_rs::io::{format_actions, parse_actions, parse_position};
use othello_complexity_rs::othello::{Board, BoardSymmetry, Color, Position, CENTER_MASK};
use othello_complexity_rs::record::{replay, PlyAction};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use support::{enumerate, ArrayPosition, ALL_SYMMETRIES};

fn results(board: Board) -> [bool; 5] {
    Filter::PIPELINE.map(|filter| filter.check(&board).unwrap().passes())
}

fn non_lp_results(board: Board) -> [bool; 4] {
    std::array::from_fn(|index| Filter::PIPELINE[index].check(&board).unwrap().passes())
}

/// Zero denotes an empty square; one and two denote the two disc colours.
fn line_successors(line: &[u8]) -> Vec<Vec<u8>> {
    let len = line.len() as isize;
    let mut successors = Vec::new();

    // A placement may capture away from this line, so either colour may be
    // placed at any empty square without requiring an on-line capture.
    for (square, &cell) in line.iter().enumerate() {
        if cell != 0 {
            continue;
        }
        for colour in [1, 2] {
            let mut next = line.to_vec();
            next[square] = colour;
            for direction in [-1, 1] {
                let mut cursor = square as isize + direction;
                let mut captured = Vec::new();
                while (0..len).contains(&cursor) && line[cursor as usize] == 3 - colour {
                    captured.push(cursor as usize);
                    cursor += direction;
                }
                if (0..len).contains(&cursor) && line[cursor as usize] == colour {
                    for captured_square in captured {
                        next[captured_square] = colour;
                    }
                }
            }
            successors.push(next);
        }
    }

    // A move outside a segment may capture its monochromatic prefix or
    // suffix. Its anchor is the next opposite-colour disc, or an exterior
    // disc when the entire segment has one colour. An empty square blocks it.
    for (start, direction) in [(0, 1), (len - 1, -1)] {
        let colour = line[start as usize];
        if colour == 0 {
            continue;
        }
        let mut cursor = start;
        let mut captured = Vec::new();
        while (0..len).contains(&cursor) && line[cursor as usize] == colour {
            captured.push(cursor as usize);
            cursor += direction;
        }
        if (0..len).contains(&cursor) && line[cursor as usize] == 0 {
            continue;
        }
        let mut next = line.to_vec();
        for captured_square in captured {
            next[captured_square] = 3 - colour;
        }
        successors.push(next);
    }

    successors
}

fn reachable_lines(len: usize) -> HashSet<Vec<u8>> {
    let empty = vec![0; len];
    let mut seen = HashSet::from([empty.clone()]);
    let mut pending = VecDeque::from([empty]);
    while let Some(line) = pending.pop_front() {
        for next in line_successors(&line) {
            if seen.insert(next.clone()) {
                pending.push_back(next);
            }
        }
    }
    seen
}

#[test]
fn edge_filters_match_the_independent_line_game_for_all_lengths_and_configurations() {
    for len in 1..=8 {
        let reachable = reachable_lines(len);
        for mut encoding in 0..3_usize.pow(len as u32) {
            let mut line = vec![0; len];
            let mut player = 0;
            let mut opponent = 0;
            for (square, cell) in line.iter_mut().enumerate() {
                *cell = (encoding % 3) as u8;
                encoding /= 3;
                // Embed the segment at A1 on an otherwise empty board.
                // No other direction contains four occupied squares.
                match *cell {
                    1 => player |= 1_u64 << square,
                    2 => opponent |= 1_u64 << square,
                    _ => {}
                }
            }
            let expected = reachable.contains(&line);
            assert_eq!(
                check_edge_patterns(player, opponent),
                expected,
                "physical edge {line:?} disagrees with the independent line game"
            );
            assert_eq!(
                check_virtual_edge_patterns(player, opponent),
                expected,
                "virtual edge {line:?} disagrees with the independent line game"
            );
        }
    }
}

#[test]
fn every_independently_reachable_position_through_nine_discs_passes_applicable_filters() {
    for position in enumerate(ArrayPosition::initial(), 9) {
        let filters = if position.discs() <= 7 {
            Filter::PIPELINE.as_slice()
        } else {
            &Filter::PIPELINE[..4]
        };
        for symmetry in ALL_SYMMETRIES {
            let image = position.transformed(symmetry);
            for position in [image, image.opposite_turn()] {
                let board = position.position().board;
                for &filter in filters {
                    assert!(
                        filter.check(&board).unwrap().passes(),
                        "filter {} rejected reachable position {:?}",
                        filter.name(),
                        position.position()
                    );
                }
            }
        }
    }
}

#[test]
fn independent_complete_games_including_passes_and_terminals_pass_applicable_filters() {
    let mut position = ArrayPosition::initial();
    let mut first_game_passes = 0;
    loop {
        assert_eq!(results(position.position().board), [true; 5]);
        let moves = position.moves();
        if let Some(&square) = moves.first() {
            position = position.apply(PlyAction::Play(square)).unwrap();
        } else if let Ok(next) = position.apply(PlyAction::Pass) {
            // Passing preserves the occupied squares and exchanges the relative
            // colours. It must not change any necessary condition.
            assert_eq!(results(next.position().board), [true; 5]);
            position = next;
            first_game_passes += 1;
        } else {
            break;
        }
    }
    assert!(
        first_game_passes > 0,
        "the first-move game must cover legal passes"
    );

    let mut rng = ChaCha8Rng::seed_from_u64(20261024);
    let mut random_game_passes = 0;
    for game in 0..256 {
        let mut position = ArrayPosition::initial();
        let mut actions = Vec::new();
        loop {
            let board = position.position().board;
            assert_eq!(
                non_lp_results(board),
                [true; 4],
                "game {game}, legal trace {}, position {:?}",
                format_actions(&actions),
                position.position()
            );
            let moves = position.moves();
            if moves.is_empty() {
                if let Ok(next) = position.apply(PlyAction::Pass) {
                    position = next;
                    actions.push(PlyAction::Pass);
                    random_game_passes += 1;
                } else {
                    break;
                }
            } else {
                let action = PlyAction::Play(moves[rng.random_range(0..moves.len())]);
                position = position.apply(action).unwrap();
                actions.push(action);
            }
        }
    }
    assert!(
        random_game_passes > 0,
        "the fixed-seed games must cover legal passes"
    );
}

#[test]
fn relaxed_filters_are_invariant_even_for_rejected_fixed_and_random_inputs() {
    let initial = ArrayPosition::initial().position().board;
    let boards = [
        Board::new(initial.player | 1, initial.opponent),
        Board::new(CENTER_MASK, 0),
        Board::new(initial.player | 0x55, initial.opponent | 0xaa),
    ];
    assert!(!check_seg3_more(
        boards[0].player,
        boards[0].opponent,
        false
    ));
    for board in boards {
        let expected = results(board);
        for symmetry in BoardSymmetry::ALL {
            let image = symmetry
                .apply_position(Position {
                    board,
                    side_to_move: Color::Black,
                })
                .board;
            assert_eq!(results(image), expected);
            assert_eq!(results(image.swapped()), expected);
        }
    }

    let mut rng = rand::rngs::StdRng::seed_from_u64(0x8a11_5eed);
    for _ in 0..1024 {
        let a = rng.random::<u64>();
        let b = rng.random::<u64>();
        for occupied in [a | CENTER_MASK, a | b | CENTER_MASK] {
            let player = b & occupied;
            let board = Board::new(player, occupied ^ player);
            let expected_occupancy = check_occupancy(occupied);
            let expected_edges = check_virtual_edge_patterns(board.player, board.opponent);
            let expected_seg3 = check_seg3_more(board.player, board.opponent, false);

            for symmetry in ALL_SYMMETRIES {
                let image = symmetry.apply(board);
                assert_eq!(
                    check_occupancy(image.player | image.opponent),
                    expected_occupancy,
                    "occupancy symmetry={symmetry:?}, board={board:?}"
                );
                assert_eq!(
                    check_virtual_edge_patterns(image.player, image.opponent),
                    expected_edges,
                    "virtual edges symmetry={symmetry:?}, board={board:?}"
                );
                assert_eq!(
                    check_seg3_more(image.player, image.opponent, false),
                    expected_seg3,
                    "seg3 symmetry={symmetry:?}, board={board:?}"
                );
            }
        }
    }
}

fn assert_passes(position: ArrayPosition) {
    let board = position.position().board;
    assert!(
        check_virtual_edge_patterns(board.player, board.opponent),
        "legal position rejected: {:?}",
        position.position()
    );
}

fn assert_legal_trace_passes(actions: &str, expected: &str) -> Position {
    let actions = parse_actions(actions).unwrap();
    let expected = parse_position(expected).unwrap();
    let oracle_trace = ArrayPosition::initial().replay(&actions).unwrap();
    let production_trace = replay(Position::initial(), &actions).unwrap();
    assert_eq!(
        production_trace,
        oracle_trace
            .iter()
            .map(|position| position.position())
            .collect::<Vec<_>>()
    );
    assert_eq!(production_trace.last(), Some(&expected));
    assert!(check_edge_patterns(
        expected.board.player,
        expected.board.opponent
    ));
    for position in oracle_trace {
        assert_passes(position);
    }
    expected
}

fn line_board(pattern: &str, start: (usize, usize), step: (i32, i32)) -> Board {
    let mut board = Board::new(CENTER_MASK, 0);
    for (offset, cell) in pattern.bytes().enumerate() {
        let x = start.0 as i32 + offset as i32 * step.0;
        let y = start.1 as i32 + offset as i32 * step.1;
        assert!((0..8).contains(&x) && (0..8).contains(&y));
        let bit = 1_u64 << (8 * y + x);
        board.player &= !bit;
        board.opponent &= !bit;
        match cell {
            b'X' => board.player |= bit,
            b'O' => board.opponent |= bit,
            b'-' => (),
            _ => panic!("invalid line fixture"),
        }
    }
    board
}

fn assert_verdict_for_images(board: Board, expected: bool) {
    let position = ArrayPosition::from_relative(&board.to_string(), true);
    for symmetry in ALL_SYMMETRIES {
        let image = position.transformed(symmetry);
        for position in [image, image.opposite_turn()] {
            let board = position.position().board;
            assert_eq!(
                check_virtual_edge_patterns(board.player, board.opponent),
                expected,
                "incorrect verdict for {:?}",
                position.position()
            );
        }
    }
}

#[test]
fn edge_pipeline_distinguishes_a_legal_alternating_line_from_a_virtual_edge() {
    // F3-F6 reads XOXO and contains no initial disc. Its neighbours permit
    // captures across the line, despite its contact with exterior empties.
    let legal = assert_legal_trace_passes(
        "E6 F4 F3 F6 F5 G4",
        "---------------------X-----OOOO----XXX------XO------------------ b",
    );
    let impossible = Position {
        board: line_board("XOXO", (2, 1), (1, 0)),
        side_to_move: Color::Black,
    };
    assert!(check_edge_patterns(
        impossible.board.player,
        impossible.board.opponent
    ));
    for (name, position, passes) in [
        ("virtual pattern", impossible, false),
        ("legal alternating line", legal, true),
    ] {
        assert_eq!(
            Filter::Edge.check(&position.board).unwrap().passes(),
            passes,
            "case={name}"
        );
    }
}

#[test]
fn forbidden_words_on_lines_without_transverse_captures_are_rejected() {
    // These three words and their colour exchanges are the forbidden edge
    // words. Empty neighbours or the board boundary block every transverse
    // capture in these fixtures; a virtual edge can be shorter than 8 cells.
    for pattern in ["XOXO", "XOXXOX", "XOXXOOXO"] {
        for (start, step) in [
            ((0, 0), (1, 0)),  // physical rank
            ((0, 1), (1, 0)),  // internal rank
            ((1, 0), (0, 1)),  // internal file
            ((0, 0), (1, 1)),  // main diagonal
            ((0, 7), (1, -1)), // other diagonal
        ] {
            assert_verdict_for_images(line_board(pattern, start, step), false);
        }
    }
}

#[test]
fn each_transverse_capture_axis_must_be_blocked() {
    let base = line_board("XOXO", (2, 1), (1, 0)); // C2-F2
    assert_verdict_for_images(base, false);
    for (first, opposite) in [(2, 18), (1, 19), (3, 17)] {
        // These are the three pairs of opposite neighbours of C2 outside
        // the horizontal line. A single empty neighbour blocks that axis.
        let first_only = Board::new(base.player | (1 << first), base.opponent);
        assert_verdict_for_images(first_only, false);
        let opposite_only = Board::new(base.player | (1 << opposite), base.opponent);
        assert_verdict_for_images(opposite_only, false);
        let both = Board::new(first_only.player | (1 << opposite), base.opponent);
        assert_verdict_for_images(both, true);
    }
}

#[test]
fn patterns_cannot_continue_across_a_board_boundary() {
    // These indices spell XOXO only if a bit shift wraps H-file to A-file,
    // or A-file to H-file. In coordinates they are not straight lines.
    for [first, second, third, fourth] in [[6, 7, 8, 9], [6, 15, 24, 33], [1, 8, 15, 22]] {
        let board = Board::new(
            CENTER_MASK | (1 << first) | (1 << third),
            (1 << second) | (1 << fourth),
        );
        assert_verdict_for_images(board, true);
    }
}
