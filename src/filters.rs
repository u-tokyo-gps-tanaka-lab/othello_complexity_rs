pub mod connectivity;
pub mod linear_programming;
pub mod never_flipped;
pub mod occupancy;
pub(crate) mod reverse;
pub mod sat;

use std::io;

use crate::othello::Board;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Con,
    Occupancy,
    NeverFlipped,
    Lp,
}

impl Filter {
    pub const PIPELINE: [Self; 4] = [Self::Con, Self::Occupancy, Self::NeverFlipped, Self::Lp];

    pub fn name(self) -> &'static str {
        match self {
            Self::Con => "con",
            Self::Occupancy => "occupancy",
            Self::NeverFlipped => "never_flipped",
            Self::Lp => "lp",
        }
    }

    pub fn check(self, board: &Board) -> io::Result<FilterResult> {
        match self {
            Self::Con => Ok(necessary_condition(
                connectivity::is_connected(board.player | board.opponent),
                "occupied squares are disconnected from the centre",
            )),
            Self::Occupancy => Ok(necessary_condition(
                occupancy::check_occupancy(board.player | board.opponent),
                "some occupied squares cannot be reached from the centre",
            )),
            Self::NeverFlipped => Ok(necessary_condition(
                never_flipped::check_never_flipped(board.player, board.opponent),
                "the possible flips cannot explain the final colours",
            )),
            Self::Lp => linear_programming::check_lp(board.player, board.opponent, false),
        }
    }
}

fn necessary_condition(passes: bool, reason: &str) -> FilterResult {
    if passes {
        FilterResult::Pass
    } else {
        FilterResult::Rejected {
            reason: reason.to_owned(),
        }
    }
}

/// 必要条件による判定。通過は到達可能性の証明を意味しない。
///
/// 未確定は理由を記録して後段へ通す。ソルバや入出力の失敗は、この型に
/// 変換せず呼出し元へ `Err` として返す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterResult {
    Pass,
    Rejected { reason: String },
    Undetermined { reason: String },
}

impl FilterResult {
    pub fn passes(&self) -> bool {
        !matches!(self, Self::Rejected { .. })
    }

    pub fn status(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Rejected { .. } => "rejected",
            Self::Undetermined { .. } => "undetermined",
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::Pass => "",
            Self::Rejected { reason } | Self::Undetermined { reason } => reason,
        }
    }
}
