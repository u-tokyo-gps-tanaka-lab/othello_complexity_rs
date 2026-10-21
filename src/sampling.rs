//! Absolute-position sampling, with multiplicities and an explicit sampling design.
//!
//! A rank's low bit is the absolute turn (0 = Black, 1 = White). The remaining
//! digits run from A1 to H8, in radix 2 at the four occupied central squares and
//! radix 3 elsewhere. Central digits 0/1 mean X/O; other digits 0/1/2 mean -/X/O.
//! Changing only the turn bit exchanges absolute colors as well as the turn.
//! No symmetry quotient or reachability condition enters this population.

use crate::othello::{validate_board, Board, Color, Position, CENTER_MASK};
pub const POPULATION_SIZE: u128 = 32 * 3_u128.pow(60);

/// Decode one rank, rejecting values outside the entire absolute population.
pub fn position_from_rank(mut rank: u128) -> Result<Position, String> {
    if rank >= POPULATION_SIZE {
        return Err(format!("rank must be less than {POPULATION_SIZE}"));
    }
    let side_to_move = if rank & 1 == 0 {
        Color::Black
    } else {
        Color::White
    };
    rank /= 2;
    let mut board = Board::empty();
    for square in 0..64 {
        let mask = 1_u64 << square;
        let central = mask & CENTER_MASK != 0;
        let radix = if central { 2 } else { 3 };
        let digit = rank % radix;
        rank /= radix;
        if digit == u128::from(!central) {
            board.player |= mask;
        } else if digit == radix - 1 {
            board.opponent |= mask;
        }
    }
    Ok(Position {
        board,
        side_to_move,
    })
}

/// Inverse of [`position_from_rank`], including both absolute turns.
pub fn rank_of_position(position: Position) -> Result<u128, String> {
    validate_board(&position.board).map_err(|error| format!("invalid board: {error:?}"))?;
    let mut rank = u128::from(position.side_to_move == Color::White);
    let mut weight = 2;
    for square in 0..64 {
        let mask = 1_u64 << square;
        let central = mask & CENTER_MASK != 0;
        let (radix, digit) = if central {
            (2, u128::from(position.board.opponent & mask != 0))
        } else {
            (
                3,
                if position.board.player & mask != 0 {
                    1
                } else if position.board.opponent & mask != 0 {
                    2
                } else {
                    0
                },
            )
        };
        rank += weight * digit;
        weight *= radix;
    }
    Ok(rank)
}
