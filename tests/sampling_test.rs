//! The mixed-radix population definition is the decoding oracle.
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
