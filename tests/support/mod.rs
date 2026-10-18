//! Independent rules oracle for the reachability contract.
//!
//! The board contains absolute colors in an array. Legal moves, captures, passes,
//! and symmetries use coordinate arithmetic, never production bitboard rules.

use std::collections::HashSet;

use othello_complexity_rs::othello::{Board, BoardSymmetry, Color, Position};
use othello_complexity_rs::record::PlyAction;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Disc {
    Black,
    White,
}

impl Disc {
    fn opposite(self) -> Self {
        match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ArrayPosition {
    cells: [Option<Disc>; 64],
    turn: Disc,
}

impl ArrayPosition {
    pub fn initial() -> Self {
        let mut cells = [None; 64];
        cells[3 * 8 + 4] = Some(Disc::Black); // E4
        cells[4 * 8 + 3] = Some(Disc::Black); // D5
        cells[3 * 8 + 3] = Some(Disc::White); // D4
        cells[4 * 8 + 4] = Some(Disc::White); // E5
        Self {
            cells,
            turn: Disc::Black,
        }
    }

    pub fn from_relative(text: &str, black_to_move: bool) -> Self {
        assert_eq!(text.len(), 64);
        let turn = if black_to_move {
            Disc::Black
        } else {
            Disc::White
        };
        let mut cells = [None; 64];
        for (square, value) in text.bytes().enumerate() {
            cells[square] = match value {
                b'X' => Some(turn),
                b'O' => Some(turn.opposite()),
                b'-' => None,
                _ => panic!("invalid oracle fixture character"),
            };
        }
        Self { cells, turn }
    }

    /// Convert only at the test boundary, preserving the relative-board contract.
    pub fn position(self) -> Position {
        let mut player = 0;
        let mut opponent = 0;
        for (square, cell) in self.cells.into_iter().enumerate() {
            if cell == Some(self.turn) {
                player |= 1_u64 << square;
            } else if cell.is_some() {
                opponent |= 1_u64 << square;
            }
        }
        Position {
            board: Board { player, opponent },
            side_to_move: match self.turn {
                Disc::Black => Color::Black,
                Disc::White => Color::White,
            },
        }
    }

    pub fn discs(self) -> usize {
        self.cells.iter().filter(|cell| cell.is_some()).count()
    }

    /// Exchange every absolute color and the turn; the relative board is fixed.
    pub fn color_exchanged(self) -> Self {
        Self {
            cells: self.cells.map(|cell| cell.map(Disc::opposite)),
            turn: self.turn.opposite(),
        }
    }

    /// Change only the absolute turn, including when that turn has no legal pass.
    pub fn opposite_turn(self) -> Self {
        Self {
            turn: self.turn.opposite(),
            ..self
        }
    }

    pub fn transformed(self, symmetry: BoardSymmetry) -> Self {
        let mut cells = [None; 64];
        for (square, cell) in self.cells.into_iter().enumerate() {
            let (x, y) = (square % 8, square / 8);
            let (u, v) = match symmetry {
                BoardSymmetry::Identity => (x, y),
                BoardSymmetry::Rotate90 => (7 - y, x),
                BoardSymmetry::Rotate180 => (7 - x, 7 - y),
                BoardSymmetry::Rotate270 => (y, 7 - x),
                BoardSymmetry::MirrorHorizontal => (7 - x, y),
                BoardSymmetry::MirrorVertical => (x, 7 - y),
                BoardSymmetry::Transpose => (y, x),
                BoardSymmetry::AntiTranspose => (7 - y, 7 - x),
            };
            cells[v * 8 + u] = cell;
        }
        Self {
            cells,
            turn: self.turn,
        }
    }

    pub fn orbit(self) -> impl Iterator<Item = Self> {
        ALL_SYMMETRIES
            .into_iter()
            .map(move |symmetry| self.transformed(symmetry))
    }

    fn captures(self, square: usize) -> Vec<usize> {
        if square >= 64 || self.cells[square].is_some() {
            return Vec::new();
        }
        let mut captures = Vec::new();
        for dy in -1..=1 {
            for dx in -1..=1 {
                if (dx, dy) == (0, 0) {
                    continue;
                }
                let mut x = square as i32 % 8 + dx;
                let mut y = square as i32 / 8 + dy;
                let mut line = Vec::new();
                while (0..8).contains(&x) && (0..8).contains(&y) {
                    let index = (y * 8 + x) as usize;
                    match self.cells[index] {
                        Some(disc) if disc != self.turn => line.push(index),
                        Some(_) => {
                            captures.extend(line);
                            break;
                        }
                        None => break,
                    }
                    x += dx;
                    y += dy;
                }
            }
        }
        captures
    }

    pub fn moves(self) -> Vec<usize> {
        (0..64)
            .filter(|&square| !self.captures(square).is_empty())
            .collect()
    }

    pub fn apply(self, action: PlyAction) -> Result<Self, String> {
        let mut next = Self {
            turn: self.turn.opposite(),
            ..self
        };
        match action {
            PlyAction::Play(square) => {
                let captured = self.captures(square);
                if captured.is_empty() {
                    return Err(format!("illegal oracle placement at {square}"));
                }
                next.cells[square] = Some(self.turn);
                for square in captured {
                    next.cells[square] = Some(self.turn);
                }
            }
            PlyAction::Pass => {
                if !self.moves().is_empty() || next.moves().is_empty() {
                    return Err("illegal oracle pass".to_string());
                }
            }
        }
        Ok(next)
    }

    pub fn replay(self, actions: &[PlyAction]) -> Result<Vec<Self>, String> {
        let mut states = vec![self];
        let mut current = self;
        for &action in actions {
            current = current.apply(action)?;
            states.push(current);
        }
        Ok(states)
    }
}

// Explicit enumeration keeps the oracle independent of the production ALL list.
pub const ALL_SYMMETRIES: [BoardSymmetry; 8] = [
    BoardSymmetry::Identity,
    BoardSymmetry::Rotate90,
    BoardSymmetry::Rotate180,
    BoardSymmetry::Rotate270,
    BoardSymmetry::MirrorHorizontal,
    BoardSymmetry::MirrorVertical,
    BoardSymmetry::Transpose,
    BoardSymmetry::AntiTranspose,
];

/// Enumerate all absolute positions while bounding placements, including a
/// legal pass at the bound. Terminal positions have no successors.
pub fn enumerate(start: ArrayPosition, max_discs: usize) -> HashSet<ArrayPosition> {
    assert!(start.discs() <= max_discs);
    let mut seen = HashSet::new();
    let mut stack = vec![start];
    while let Some(position) = stack.pop() {
        if !seen.insert(position) {
            continue;
        }
        let moves = position.moves();
        if moves.is_empty() {
            if let Ok(after_pass) = position.apply(PlyAction::Pass) {
                stack.push(after_pass);
            }
        } else if position.discs() < max_discs {
            for square in moves {
                stack.push(position.apply(PlyAction::Play(square)).unwrap());
            }
        }
    }
    seen
}

/// All eight geometric images with both possible absolute turns for each
/// relative board. Hashing removes duplicate queries, not reachability states.
pub fn query_targets(reachable: &HashSet<ArrayPosition>) -> Vec<ArrayPosition> {
    let targets: HashSet<_> = reachable
        .iter()
        .flat_map(|&position| position.orbit())
        .flat_map(|position| [position, position.color_exchanged()])
        .collect();
    let mut targets: Vec<_> = targets.into_iter().collect();
    targets.sort_by_key(|position| {
        let position = position.position();
        (
            position.board.player,
            position.board.opponent,
            position.side_to_move == Color::White,
        )
    });
    targets
}

/// A legal 62-stone position with the forced three-ply line H2, pass, H7.
pub fn middle_pass_start() -> ArrayPosition {
    ArrayPosition::from_relative(
        "OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO",
        false,
    )
}
