pub mod connectivity;
pub mod flip_sat;
pub mod impossible_edges;
pub mod layer_sat;
pub mod linear_programming;
pub mod occupancy;
pub mod seg3;

use std::io;

use crate::othello::Board;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Con,
    Edge,
    Occupancy,
    Seg3More,
    Lp,
}

impl Filter {
    pub const PIPELINE: [Self; 5] = [
        Self::Con,
        Self::Edge,
        Self::Occupancy,
        Self::Seg3More,
        Self::Lp,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Con => "con",
            Self::Edge => "edge",
            Self::Occupancy => "occupancy",
            Self::Seg3More => "seg3more",
            Self::Lp => "lp",
        }
    }

    pub fn check(self, board: &Board) -> io::Result<FilterResult> {
        match self {
            Self::Con => Ok(necessary_condition(
                connectivity::is_connected(board.player | board.opponent),
                "occupied squares are disconnected from the centre",
            )),
            Self::Edge => Ok(necessary_condition(
                impossible_edges::check_edge_patterns(board.player, board.opponent),
                "an impossible physical-edge pattern is present",
            )),
            Self::Occupancy => Ok(necessary_condition(
                occupancy::check_occupancy(board.player | board.opponent),
                "some occupied squares cannot be reached from the centre",
            )),
            Self::Seg3More => {
                if !occupancy::check_occupancy(board.player | board.opponent) {
                    return Ok(necessary_condition(
                        false,
                        "occupancy reachability prerequisite is not satisfied",
                    ));
                }
                Ok(necessary_condition(
                    seg3::check_seg3_more(board.player, board.opponent, false),
                    "the possible flips cannot explain the final colours",
                ))
            }
            Self::Lp => linear_programming::check_lp(board.player, board.opponent, false, false),
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
