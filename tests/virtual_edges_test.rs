//! Virtual edges are a necessary condition: every independently generated
//! legal game must pass. Geometric and colour invariance belong to this relaxed
//! condition, not to exact reachability from the fixed standard initial state.

#[allow(dead_code)]
mod support;

use othello_complexity_rs::filters::impossible_edges::{
    check_edge_patterns, check_virtual_edge_patterns,
};
use othello_complexity_rs::filters::Filter;
use othello_complexity_rs::io::{format_actions, parse_actions, parse_position};
use othello_complexity_rs::othello::{Board, Color, Position, CENTER_MASK};
use othello_complexity_rs::record::{replay, PlyAction};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use support::{enumerate, ArrayPosition, ALL_SYMMETRIES};

fn assert_passes(position: ArrayPosition) {
    let board = position.position().board;
    assert!(
        check_virtual_edge_patterns(board.player, board.opponent),
        "legal position rejected: {:?}",
        position.position()
    );
}

#[test]
fn alternating_line_through_initial_discs_can_be_reached() {
    // D2-D5 reads XOXO, including the initial discs at D4 and D5.
    // Adjacency to exterior empty squares does not make this line an edge.
    assert_legal_trace_passes(
        "D3 C3 B3 D2 C2",
        "----------OX-----OOO-------XO------OX--------------------------- w",
    );
}

#[test]
fn alternating_line_away_from_initial_discs_can_be_reached() {
    // F3-F6 reads XOXO and contains no initial disc. Its neighbours permit
    // captures across the line, despite its contact with exterior empties.
    assert_legal_trace_passes(
        "E6 F4 F3 F6 F5 G4",
        "---------------------X-----OOOO----XXX------XO------------------ b",
    );
}

fn assert_legal_trace_passes(actions: &str, expected: &str) {
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
fn short_words_and_words_interrupted_by_empty_squares_pass() {
    for pattern in ["XOX", "OXO", "XO-XO"] {
        assert_verdict_for_images(line_board(pattern, (2, 1), (1, 0)), true);
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

#[test]
fn edge_pipeline_classifies_virtual_patterns_and_legal_positions() {
    let impossible = Position {
        board: line_board("XOXO", (2, 1), (1, 0)),
        side_to_move: Color::Black,
    };
    assert!(check_edge_patterns(
        impossible.board.player,
        impossible.board.opponent
    ));
    let legal =
        parse_position("---------------------X-----OOOO----XXX------XO------------------ b")
            .unwrap();
    for (position, passes) in [(impossible, false), (legal, true)] {
        assert_eq!(
            Filter::Edge.check(&position.board).unwrap().passes(),
            passes
        );
    }
}

#[test]
fn every_independently_reachable_position_through_nine_discs_passes() {
    for position in enumerate(ArrayPosition::initial(), 9) {
        assert_passes(position);
        for symmetry in ALL_SYMMETRIES {
            let image = position.transformed(symmetry);
            assert_passes(image);
            assert_passes(image.opposite_turn());
        }
    }
}

#[test]
fn independent_complete_games_including_passes_and_terminals_pass() {
    let mut rng = ChaCha8Rng::seed_from_u64(20261024);
    let mut passes = 0;
    for game in 0..256 {
        let mut position = ArrayPosition::initial();
        let mut actions = Vec::new();
        loop {
            let board = position.position().board;
            assert!(
                check_virtual_edge_patterns(board.player, board.opponent),
                "game {game}, legal trace {}, position {:?}",
                format_actions(&actions),
                position.position()
            );
            let moves = position.moves();
            if moves.is_empty() {
                if let Ok(next) = position.apply(PlyAction::Pass) {
                    position = next;
                    actions.push(PlyAction::Pass);
                    passes += 1;
                } else {
                    assert!(position.opposite_turn().moves().is_empty());
                    break;
                }
            } else {
                let action = PlyAction::Play(moves[rng.random_range(0..moves.len())]);
                position = position.apply(action).unwrap();
                actions.push(action);
            }
        }
    }
    assert!(passes > 0, "the sample must exercise legal passes");
}
