//! Necessary conditions from the absolute-position pipeline contract:
//! independent legal play must pass, and colour/turn/geometric changes must
//! preserve these deliberately relaxed conditions.

#[allow(dead_code)]
mod support;

use othello_complexity_rs::filters::{seg3::check_seg3_more, Filter};
use othello_complexity_rs::othello::{Board, BoardSymmetry, Color, Position, CENTER_MASK};
use othello_complexity_rs::record::PlyAction;
use support::{enumerate, ArrayPosition, ALL_SYMMETRIES};

fn results(board: Board) -> [bool; 5] {
    Filter::PIPELINE.map(|filter| filter.check(&board).unwrap().passes())
}

#[test]
fn every_independently_reachable_position_through_seven_discs_passes_every_filter() {
    for position in enumerate(ArrayPosition::initial(), 7) {
        let expected = [true; 5];
        for symmetry in ALL_SYMMETRIES {
            let image = position.transformed(symmetry);
            assert_eq!(
                results(image.position().board),
                expected,
                "reachable relative board rejected: {:?}",
                image.position()
            );
            assert_eq!(results(image.opposite_turn().position().board), expected);
            assert_eq!(results(image.color_exchanged().position().board), expected);
        }
    }
}

#[test]
fn a_complete_independent_game_including_passes_and_terminal_passes_every_filter() {
    let mut position = ArrayPosition::initial();
    let mut passes = 0;
    let mut placements = 0;
    loop {
        assert_eq!(results(position.position().board), [true; 5]);
        let moves = position.moves();
        if let Some(&square) = moves.first() {
            position = position.apply(PlyAction::Play(square)).unwrap();
            placements += 1;
        } else if let Ok(next) = position.apply(PlyAction::Pass) {
            // Passing preserves the occupied squares and exchanges the relative
            // colours. It must not change any necessary condition.
            assert_eq!(results(next.position().board), [true; 5]);
            position = next;
            passes += 1;
        } else {
            assert!(position.opposite_turn().moves().is_empty());
            assert!(position.apply(PlyAction::Pass).is_err());
            break;
        }
    }
    assert!(
        passes > 0,
        "this independently generated game must cover legal passes"
    );
    assert!(placements > 0);
}

#[test]
fn relaxed_conditions_are_invariant_even_for_inputs_they_reject() {
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
}
