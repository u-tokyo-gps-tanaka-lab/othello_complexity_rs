use crate::filters::occupancy::{occupancy_order, occupancy_order_cached};
use crate::filters::FilterResult;
use crate::othello::{validate_board, Board, Direction, CENTER_MASK};
use highs::{HighsModelStatus, RowProblem, Sense};
use std::io;

/// 制約の種類
#[derive(Clone, Copy)]
enum CstrSense {
    Le,
    Ge,
    Eq,
}

/// 1行の疎制約:  Σ_j a[j]*x[col[j]] (<=|=|>=) rhs
struct SparseConstraint {
    cols: Vec<i32>,   // 列インデックス（0-based）
    vals: Vec<f64>,   // 係数
    sense: CstrSense, // <=, >=, ==
    rhs: f64,         // 右辺
}

#[derive(Clone, Copy)]
enum VariableKind {
    Unit,
    Free,
}

struct VarMaker {
    kinds: Vec<VariableKind>,
}

impl VarMaker {
    fn new() -> Self {
        Self { kinds: vec![] }
    }

    fn mk_var(&mut self) -> i32 {
        self.add(VariableKind::Unit)
    }

    fn mk_free_var(&mut self) -> i32 {
        self.add(VariableKind::Free)
    }

    fn add(&mut self, kind: VariableKind) -> i32 {
        let index = self.kinds.len();
        self.kinds.push(kind);
        index as i32
    }
}

fn merged_constraint(
    terms: impl IntoIterator<Item = (i32, f64)>,
    sense: CstrSense,
    rhs: f64,
) -> SparseConstraint {
    let mut merged: Vec<(i32, f64)> = vec![];
    for (column, value) in terms {
        if let Some((_, coefficient)) = merged.iter_mut().find(|(seen, _)| *seen == column) {
            *coefficient += value;
        } else {
            merged.push((column, value));
        }
    }
    merged.retain(|(_, value)| *value != 0.0);
    let (cols, vals) = merged.into_iter().unzip();
    SparseConstraint {
        cols,
        vals,
        sense,
        rhs,
    }
}

#[inline]
fn xy2sq(x: i32, y: i32) -> usize {
    (y * 8 + x) as usize
}
/// 連続緩和で可否のみ判定 (HiGHS 1.12.0 API)
/// by_ip_solver: falseが望ましい
fn check_feasibility(
    variable_kinds: &[VariableKind],
    constraints: &[SparseConstraint],
    by_ip_solver: bool,
) -> io::Result<FilterResult> {
    // 変数→制約の順に作るので RowProblem を使う
    let mut pb = RowProblem::default();

    // 目的係数は 0（可否だけ確認）
    let cols: Vec<_> = variable_kinds
        .iter()
        .map(|kind| {
            let (lower, upper, is_integer) = match kind {
                VariableKind::Unit => (0.0, 1.0, by_ip_solver),
                VariableKind::Free => (f64::NEG_INFINITY, f64::INFINITY, false),
            };
            pb.add_column_with_integrality(0.0, lower..=upper, is_integer)
        })
        .collect();

    // 疎制約を追加
    for row in constraints {
        let terms: Vec<_> = row
            .cols
            .iter()
            .zip(&row.vals)
            .map(|(&c, &v)| (cols[c as usize], v))
            .collect();

        match row.sense {
            CstrSense::Le => {
                pb.add_row(..=row.rhs, &terms);
            } // ≤
            CstrSense::Ge => {
                pb.add_row(row.rhs.., &terms);
            } // ≥
            CstrSense::Eq => {
                pb.add_row(row.rhs..=row.rhs, &terms);
            } // ＝
        }
    }

    let mut model = pb
        .try_optimise(Sense::Minimise)
        .map_err(|status| io::Error::other(format!("HiGHS model creation failed: {status:?}")))?;
    model.make_quiet();
    model.set_option("threads", 1);
    model.set_option("presolve", "on");
    let solved = model
        .try_solve()
        .map_err(|status| io::Error::other(format!("HiGHS solve failed: {status:?}")))?;
    classify_highs_status(solved.status())
}

fn classify_highs_status(status: HighsModelStatus) -> io::Result<FilterResult> {
    match status {
        HighsModelStatus::Optimal
        | HighsModelStatus::ObjectiveTarget
        | HighsModelStatus::ObjectiveBound
        | HighsModelStatus::Unbounded => Ok(FilterResult::Pass),
        HighsModelStatus::Infeasible => Ok(FilterResult::Rejected {
            reason: "HiGHS proved the relaxation infeasible".to_owned(),
        }),
        HighsModelStatus::UnboundedOrInfeasible
        | HighsModelStatus::ReachedTimeLimit
        | HighsModelStatus::ReachedIterationLimit
        | HighsModelStatus::Unknown => Ok(FilterResult::Undetermined {
            reason: format!("HiGHS status: {status:?}"),
        }),
        HighsModelStatus::NotSet
        | HighsModelStatus::LoadError
        | HighsModelStatus::ModelError
        | HighsModelStatus::PresolveError
        | HighsModelStatus::SolveError
        | HighsModelStatus::PostsolveError
        | HighsModelStatus::ModelEmpty => Err(io::Error::other(format!(
            "HiGHS did not solve the relaxation: {status:?}"
        ))),
    }
}

/// 手番と配置の向きを固定しない反転整合性の緩和を解く。
///
/// 確定した実行不能性だけで棄却し、時間上限などの未確定には理由を付ける。
/// モデルの構築や求解の失敗は、到達不能判定に変換せずエラーとして返す。
pub fn check_lp(
    player: u64,
    opponent: u64,
    by_ip_solver: bool,
    use_occupancy_cache: bool,
) -> io::Result<FilterResult> {
    validate_board(&Board::new(player, opponent)).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid board: {error:?}"),
        )
    })?;
    let occupied = player | opponent;
    let order: [u64; 64] = if use_occupancy_cache {
        occupancy_order_cached(occupied)
    } else {
        occupancy_order(occupied)
    };
    let mut vm = VarMaker::new();
    let mut constraints = vec![];
    // First[sq][col] : sqにcolの石を置いたかを表す論理変数
    let mut first: Vec<Vec<i32>> = vec![vec![-1; 2]; 64];
    // center
    first[3 * 8 + 3][0] = vm.mk_var();
    first[3 * 8 + 3][1] = vm.mk_var();
    let vals = vec![1.0, 1.0];
    let cols = vec![first[3 * 8 + 3][0], first[3 * 8 + 3][1]];
    constraints.push(SparseConstraint {
        cols,
        vals,
        sense: CstrSense::Eq,
        rhs: 1.0,
    });
    first[3 * 8 + 4][0] = first[3 * 8 + 3][1];
    first[3 * 8 + 4][1] = first[3 * 8 + 3][0];
    first[4 * 8 + 3][0] = first[3 * 8 + 3][1];
    first[4 * 8 + 3][1] = first[3 * 8 + 3][0];
    first[4 * 8 + 4][0] = first[3 * 8 + 3][0];
    first[4 * 8 + 4][1] = first[3 * 8 + 3][1];

    // Flip[sq][col] : [F_(sq, col, d, len)], sqをcolにflipするflip全体
    let mut flip: Vec<Vec<Vec<i32>>> = vec![vec![vec![]; 2]; 64];

    // Base[sq][col] : [F_(sq', col, d, len)], sqがcolであることを利用してcolにflipするflip
    let mut base: Vec<Vec<Vec<i32>>> = vec![vec![vec![]; 2]; 64];

    // set flip, base
    let mut b = occupied & !CENTER_MASK;
    while b != 0 {
        let sq = b.trailing_zeros() as usize; // 0..=63
        b &= b - 1;
        first[sq][0] = vm.mk_var();
        first[sq][1] = vm.mk_var();
        let vals = vec![1.0, 1.0];
        let cols = vec![first[sq][0], first[sq][1]];
        constraints.push(SparseConstraint {
            cols,
            vals,
            sense: CstrSense::Eq,
            rhs: 1.0,
        });
        let o = order[sq];
        let x = (sq % 8) as i32;
        let y = (sq / 8) as i32;
        for col in 0..2 {
            let mut line_vars: Vec<Vec<i32>> = vec![vec![]; 8];
            for (dir, direction) in Direction::all().iter().enumerate() {
                let (dx, dy) = direction.to_offset();
                let mut sqs: Vec<usize> = vec![];
                let mut rl = 1;
                let mut x1 = x + dx;
                let mut y1 = y + dy;
                while 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && (o & (1 << xy2sq(x1, y1))) != 0 {
                    rl += 1;
                    let sq1 = xy2sq(x1, y1);
                    if rl >= 3 {
                        let v = vm.mk_var();
                        line_vars[dir].push(v);
                        for &sq2 in &sqs {
                            flip[sq2][col].push(v);
                        }
                        base[sq1][col].push(v);
                    }
                    sqs.push(sq1);
                    x1 += dx;
                    y1 += dy;
                }
            }
            let mut cols = vec![];
            for direction_vars in &line_vars {
                cols.extend_from_slice(direction_vars);
            }
            let mut vals = vec![1.0; cols.len()];
            cols.push(first[sq][col]);
            vals.push(-1.0);
            constraints.push(SparseConstraint {
                cols,
                vals,
                sense: CstrSense::Ge,
                rhs: 0.0,
            });
            for direction_vars in &line_vars {
                if !direction_vars.is_empty() {
                    let mut cols = direction_vars.clone();
                    let mut vals = vec![1.0; cols.len()];
                    cols.push(first[sq][col]);
                    vals.push(-1.0);
                    constraints.push(SparseConstraint {
                        cols,
                        vals,
                        sense: CstrSense::Le,
                        rhs: 0.0,
                    });
                }
            }
        }
    }

    let mut ever = vec![vec![None; 2]; 64];
    let mut b = occupied;
    while b != 0 {
        let sq = b.trailing_zeros() as usize;
        b &= b - 1;
        for col in 0..2 {
            if !flip[sq][col].is_empty() {
                let v = vm.mk_free_var();
                ever[sq][col] = Some(v);
                let mut cols = vec![v, first[sq][col]];
                let mut vals = vec![1.0, -1.0];
                for &flip_var in &flip[sq][col] {
                    cols.push(flip_var);
                    vals.push(-1.0);
                }
                constraints.push(SparseConstraint {
                    cols,
                    vals,
                    sense: CstrSense::Eq,
                    rhs: 0.0,
                });
            }
        }
    }

    // for all square
    let mut b = occupied;
    while b != 0 {
        let sq = b.trailing_zeros() as usize; // 0..=63
        b &= b - 1;
        // flip count constraints
        let col = if player & (1 << sq) != 0 { 0 } else { 1 };
        let final_ever = ever[sq][col].unwrap_or(first[sq][col]);
        let other_ever = ever[sq][1 - col].unwrap_or(first[sq][1 - col]);
        constraints.push(merged_constraint(
            [
                (final_ever, 2.0),
                (first[sq][col], -1.0),
                (other_ever, -2.0),
                (first[sq][1 - col], 1.0),
            ],
            CstrSense::Eq,
            1.0,
        ));
        // base constriants
        for col in 0..2 {
            for &v in &base[sq][col] {
                let ehat = ever[sq][col].unwrap_or(first[sq][col]);
                constraints.push(SparseConstraint {
                    cols: vec![ehat, v],
                    vals: vec![1.0, -1.0],
                    sense: CstrSense::Ge,
                    rhs: 0.0,
                });
            }
        }
    }
    check_feasibility(&vm.kinds, &constraints, by_ip_solver)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rejected_in_both_solver_modes(board: Board) {
        for by_ip_solver in [false, true] {
            let result = check_lp(board.player, board.opponent, by_ip_solver, false).unwrap();
            assert!(
                matches!(&result, FilterResult::Rejected { .. }),
                "expected rejection for {board:?} with by_ip_solver={by_ip_solver}, got {result:?}"
            );
        }
    }

    #[test]
    fn merged_constraint_merges_repeated_columns_and_exact_cancellation() {
        let row = merged_constraint(
            [(7, 1.0), (3, 2.0), (7, 3.0), (3, -2.0)],
            CstrSense::Eq,
            5.0,
        );
        assert_eq!(row.cols, vec![7]);
        assert_eq!(row.vals, vec![4.0]);
        assert!(matches!(row.sense, CstrSense::Eq));
        assert_eq!(row.rhs, 5.0);
    }

    #[test]
    fn rejects_four_monochrome_centres_in_both_solver_modes() {
        // Adjacent centre aliases require both a - b = 1 and b - a = 1.
        assert_rejected_in_both_solver_modes(Board::new(CENTER_MASK, 0));
    }

    #[test]
    fn rejects_an_isolated_corner_in_both_solver_modes() {
        // Empty C4 rows force both corner first variables to zero, contradicting C1.
        let initial = Board::initial();
        assert_rejected_in_both_solver_modes(Board::new(initial.player | 1, initial.opponent));
    }

    #[test]
    fn only_proven_infeasibility_rejects_a_relaxation() {
        assert!(!classify_highs_status(HighsModelStatus::Infeasible)
            .unwrap()
            .passes());
        for status in [
            HighsModelStatus::UnboundedOrInfeasible,
            HighsModelStatus::ReachedTimeLimit,
            HighsModelStatus::ReachedIterationLimit,
            HighsModelStatus::Unknown,
        ] {
            let result = classify_highs_status(status).unwrap();
            assert!(result.passes());
            assert_eq!(result.status(), "undetermined");
            assert!(!result.reason().is_empty());
        }
        for status in [
            HighsModelStatus::NotSet,
            HighsModelStatus::LoadError,
            HighsModelStatus::ModelError,
            HighsModelStatus::PresolveError,
            HighsModelStatus::SolveError,
            HighsModelStatus::PostsolveError,
            HighsModelStatus::ModelEmpty,
        ] {
            assert!(classify_highs_status(status).is_err());
        }
    }
}
