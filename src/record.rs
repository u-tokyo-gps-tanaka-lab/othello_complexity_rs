//! 絶対手番を保持した到達証跡と、その合法な再生。
//!
//! SAT変数や節生成器を参照せず、各行動を盤面から検査する。

use crate::othello::{flip, get_moves, validate_board, Board, BoardSymmetry, Position};
use std::fmt;
use std::str::FromStr;

pub use crate::io::{format_actions, parse_actions};

/// 開始局面から要求した目標へ到達するときの一致条件。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum GoalMatch {
    /// 要求した相対盤面と絶対手番に完全一致する。
    Exact,
    /// 目標の8対称像と、各像の絶対色・手番をともに交換した局面のいずれかに一致する。
    EquivalenceClass,
}

impl GoalMatch {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::EquivalenceClass => "equivalence",
        }
    }

    /// 問い合わせが許す変換を列挙する。色交換は幾何変換の後に適用する。
    pub fn transforms(self) -> impl Iterator<Item = PositionTransform> {
        [false, true]
            .into_iter()
            .flat_map(|exchange_colors| {
                BoardSymmetry::ALL
                    .into_iter()
                    .map(move |symmetry| PositionTransform {
                        symmetry,
                        exchange_colors,
                    })
            })
            .filter(move |transform| match self {
                Self::Exact => *transform == PositionTransform::IDENTITY,
                Self::EquivalenceClass => true,
            })
    }
}

impl fmt::Display for GoalMatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 幾何変換と、任意の絶対色・手番の同時交換。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct PositionTransform {
    pub symmetry: BoardSymmetry,
    pub exchange_colors: bool,
}

impl PositionTransform {
    pub const IDENTITY: Self = Self {
        symmetry: BoardSymmetry::Identity,
        exchange_colors: false,
    };

    pub fn apply_position(self, position: Position) -> Position {
        let mut transformed = self.symmetry.apply_position(position);
        if self.exchange_colors {
            transformed.side_to_move = transformed.side_to_move.opposite();
        }
        transformed
    }
}

impl fmt::Display for PositionTransform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.symmetry)?;
        if self.exchange_colors {
            formatter.write_str("+color_exchange")?;
        }
        Ok(())
    }
}

impl FromStr for PositionTransform {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (symmetry, exchange_colors) = match value.strip_suffix("+color_exchange") {
            Some(symmetry) => (symmetry, true),
            None => (value, false),
        };
        Ok(Self {
            symmetry: symmetry.parse()?,
            exchange_colors,
        })
    }
}

/// 1回の合法着手または明示されたパス。着手位置は A1 を0とする。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PlyAction {
    Play(usize),
    Pass,
}

/// 問い合わせと、それを満たす手順を対応づける到達証跡。
///
/// `input_index` は入力局面ファイルの1から始まる行番号。
/// `transform` は要求した `goal` を実際の終点へ写し、完全一致では恒等変換に限る。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReachabilityRecord {
    pub input_index: usize,
    pub start: Position,
    pub goal: Position,
    pub goal_match: GoalMatch,
    pub transform: PositionTransform,
    pub actions: Vec<PlyAction>,
}

impl ReachabilityRecord {
    /// 問い合わせ条件と全行動を検査し、開始局面と各遷移後の局面を返す。
    pub fn verify(&self) -> Result<Vec<Position>, String> {
        if self.input_index == 0 {
            return Err("input index must be at least 1".to_owned());
        }
        validate_board(&self.goal.board)
            .map_err(|error| format!("invalid goal board: {error:?}"))?;
        if !self
            .goal_match
            .transforms()
            .any(|transform| transform == self.transform)
        {
            return Err(format!(
                "{} matching does not allow transform {}",
                self.goal_match, self.transform
            ));
        }
        let positions = replay(self.start, &self.actions)?;
        let expected = self.transform.apply_position(self.goal);
        if positions.last() != Some(&expected) {
            return Err("record endpoint does not match the selected goal position".to_owned());
        }
        Ok(positions)
    }

    /// 見出しと改行を含まない6列のCSVを返す。空手順では末尾の空欄を保持する。
    pub fn to_csv(&self) -> String {
        crate::io::format_reachability_record(self)
    }

    /// 6列のCSVを1レコード読み込み、局面と識別名の構文を検査する。
    ///
    /// 行動の合法性と実際の終点は [`Self::verify`] で検証する。
    pub fn from_csv(text: &str) -> Result<Self, String> {
        crate::io::parse_reachability_record(text)
    }
}

impl TryFrom<&csv::StringRecord> for ReachabilityRecord {
    type Error = String;

    fn try_from(fields: &csv::StringRecord) -> Result<Self, Self::Error> {
        crate::io::parse_reachability_fields(fields)
    }
}

/// 全着手と全パスを順に適用し、開始局面を含む局面列を返す。
///
/// パスは手番側に合法手がなく相手側には合法手がある場合に限る。
/// 両者に合法手がない終局からの行動と、手順にないパスの補完は認めない。
/// 空手順は終局を含めて受理する。各行動で相対盤面の視点と絶対手番を交換する。
pub fn replay(start: Position, actions: &[PlyAction]) -> Result<Vec<Position>, String> {
    validate_board(&start.board).map_err(|error| format!("invalid start board: {error:?}"))?;
    let mut current = start;
    let mut positions = vec![start];
    for (index, action) in actions.iter().enumerate() {
        let Board { player, opponent } = current.board;
        let board = match *action {
            PlyAction::Play(square) => {
                if square >= 64 {
                    return Err(format!(
                        "action {}: square {square} is outside the board",
                        index + 1
                    ));
                }
                let flips = flip(square, player, opponent);
                if flips == 0 {
                    return Err(format!(
                        "action {}: illegal move at square {square}",
                        index + 1
                    ));
                }
                Board::new(opponent & !flips, player | flips | (1_u64 << square))
            }
            PlyAction::Pass => {
                if get_moves(player, opponent) != 0 {
                    return Err(format!(
                        "action {}: pass while a legal move exists",
                        index + 1
                    ));
                }
                if get_moves(opponent, player) == 0 {
                    return Err(format!(
                        "action {}: pass after the game has ended",
                        index + 1
                    ));
                }
                current.board.swapped()
            }
        };
        current = Position {
            board,
            side_to_move: current.side_to_move.opposite(),
        };
        positions.push(current);
    }
    Ok(positions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::othello::{Color, CENTER_MASK};

    fn exact_record(
        start: Position,
        goal: Position,
        actions: Vec<PlyAction>,
    ) -> ReachabilityRecord {
        ReachabilityRecord {
            input_index: 1,
            start,
            goal,
            goal_match: GoalMatch::Exact,
            transform: PositionTransform::IDENTITY,
            actions,
        }
    }

    fn before_middle_pass() -> Position {
        // Only A1 and A2 are empty; only B1 and B2 are white. Black A2
        // flips B2, leaving white no move, then black can play A1 after a pass.
        let empty = (1_u64 << 0) | (1_u64 << 8);
        let white = (1_u64 << 1) | (1_u64 << 9);
        Position {
            board: Board::new(!(empty | white), white),
            side_to_move: Color::Black,
        }
    }

    fn forced_pass() -> Position {
        // A1 is empty and B1 is the only white disc. Only black can play A1.
        Position {
            board: Board::new(1_u64 << 1, !3_u64),
            side_to_move: Color::White,
        }
    }

    #[test]
    fn legal_first_move_swaps_relative_view_and_absolute_turn() {
        // Black D3 captures D4; only E5 remains white.
        let goal = Position {
            board: Board::new(
                1_u64 << 36,
                (1_u64 << 19) | (1_u64 << 27) | (1_u64 << 28) | (1_u64 << 35),
            ),
            side_to_move: Color::White,
        };
        let record = exact_record(Position::initial(), goal, vec![PlyAction::Play(19)]);
        assert_eq!(record.verify().unwrap(), vec![Position::initial(), goal]);
        let mut forged = record;
        forged.goal.side_to_move = Color::Black;
        assert!(forged.verify().is_err());
    }

    #[test]
    fn zero_actions_allow_equal_positions_including_terminal_positions() {
        let terminal = Position {
            board: Board::new(u64::MAX, 0),
            side_to_move: Color::White,
        };
        for position in [Position::initial(), terminal] {
            let record = exact_record(position, position, Vec::new());
            assert_eq!(record.verify().unwrap(), vec![position]);
        }
        assert!(replay(terminal, &[PlyAction::Pass]).is_err());
        assert!(replay(terminal, &[PlyAction::Play(0)]).is_err());
    }

    #[test]
    fn pass_only_and_leading_middle_trailing_passes_are_explicit() {
        let start = before_middle_pass();
        let passed = Position {
            board: Board::new(!3_u64, 1_u64 << 1),
            side_to_move: Color::Black,
        };
        let terminal = Position {
            board: Board::new(0, u64::MAX),
            side_to_move: Color::White,
        };
        let cases = [
            (forced_pass(), passed, vec![PlyAction::Pass]),
            (
                forced_pass(),
                terminal,
                vec![PlyAction::Pass, PlyAction::Play(0)],
            ),
            (start, passed, vec![PlyAction::Play(8), PlyAction::Pass]),
            (
                start,
                terminal,
                vec![PlyAction::Play(8), PlyAction::Pass, PlyAction::Play(0)],
            ),
        ];
        for (start, goal, actions) in cases {
            let record = exact_record(start, goal, actions);
            let parsed = ReachabilityRecord::from_csv(&record.to_csv()).unwrap();
            assert_eq!(parsed, record);
            assert_eq!(parsed.verify().unwrap().last(), Some(&goal));
        }
    }

    #[test]
    fn replay_rejects_missing_extra_and_terminal_passes() {
        assert!(replay(forced_pass(), &[PlyAction::Play(0)]).is_err());
        assert!(replay(
            before_middle_pass(),
            &[PlyAction::Play(8), PlyAction::Play(0)]
        )
        .is_err());
        assert!(replay(Position::initial(), &[PlyAction::Pass]).is_err());
        assert!(replay(forced_pass(), &[PlyAction::Pass, PlyAction::Pass]).is_err());
        assert!(replay(
            forced_pass(),
            &[PlyAction::Pass, PlyAction::Play(0), PlyAction::Pass]
        )
        .is_err());
        for square in [0, 27, 64, usize::MAX] {
            assert!(replay(Position::initial(), &[PlyAction::Play(square)]).is_err());
        }
        let mut incorrect_turn = exact_record(
            forced_pass(),
            Position {
                board: forced_pass().board.swapped(),
                side_to_move: Color::Black,
            },
            vec![PlyAction::Pass],
        );
        incorrect_turn.goal.side_to_move = Color::White;
        assert!(incorrect_turn.verify().is_err());
    }

    #[test]
    fn records_bind_the_goal_to_the_declared_matching_condition_and_symmetry() {
        let initial = Position::initial();
        let reflected = Position {
            board: initial.board.swapped(),
            side_to_move: Color::Black,
        };
        let mut record = ReachabilityRecord {
            goal_match: GoalMatch::EquivalenceClass,
            transform: PositionTransform {
                symmetry: BoardSymmetry::MirrorHorizontal,
                exchange_colors: false,
            },
            ..exact_record(initial, reflected, Vec::new())
        };
        assert_eq!(record.verify().unwrap(), vec![initial]);
        record.transform.symmetry = BoardSymmetry::Identity;
        assert!(record.verify().is_err());
        record.transform.symmetry = BoardSymmetry::MirrorHorizontal;
        record.goal_match = GoalMatch::Exact;
        assert!(record.verify().is_err());
        record.goal_match = GoalMatch::EquivalenceClass;
        record.goal.side_to_move = Color::White;
        assert!(record.verify().is_err());
    }

    #[test]
    fn records_reject_invalid_typed_boards_and_input_numbers() {
        for board in [Board::empty(), Board::new(CENTER_MASK, CENTER_MASK)] {
            let position = Position {
                board,
                side_to_move: Color::Black,
            };
            assert!(replay(position, &[]).is_err());
            assert!(exact_record(Position::initial(), position, Vec::new())
                .verify()
                .is_err());
        }
        let mut record = exact_record(Position::initial(), Position::initial(), Vec::new());
        record.input_index = 0;
        assert!(record.verify().is_err());
    }
}
