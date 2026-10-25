use crate::filters::occupancy::{occupancy_order, occupancy_order_cached};
use crate::filters::FilterResult;
use crate::othello::{validate_board, Board, Direction, CENTER_MASK};
use bigdecimal::num_bigint::{BigInt, Sign};
use highs::{HighsModelStatus, RowProblem, Sense, SolvedModel};
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

struct VarMaker {
    count: usize,
}

impl VarMaker {
    fn new() -> Self {
        Self { count: 0 }
    }

    fn mk_var(&mut self) -> i32 {
        let index = self.count;
        self.count += 1;
        index as i32
    }
}

#[inline]
fn xy2sq(x: i32, y: i32) -> usize {
    (y * 8 + x) as usize
}
/// 連続緩和(0<=x<=1)で可否のみ判定 (HiGHS 1.12.0 API)
/// by_ip_solver: falseが望ましい
fn check_feasibility(
    n_vars: usize,
    constraints: &[SparseConstraint],
    by_ip_solver: bool,
) -> io::Result<FilterResult> {
    // 変数→制約の順に作るので RowProblem を使う
    let mut pb = RowProblem::default();

    // 目的係数は 0（可否だけ確認）
    // 各 x_i の下限・上限は [0, 1]
    let cols: Vec<_> = (0..n_vars)
        .map(|_| {
            if by_ip_solver {
                pb.add_integer_column(0.0, 0.0..=1.0)
            } else {
                pb.add_column(0.0, 0.0..=1.0)
            }
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
    let mut solved = model
        .try_solve()
        .map_err(|status| io::Error::other(format!("HiGHS solve failed: {status:?}")))?;
    let status = solved.status();
    if status == HighsModelStatus::Infeasible {
        Ok(certify_highs_infeasibility(
            &mut solved,
            n_vars,
            constraints,
        ))
    } else {
        classify_highs_status(status)
    }
}

/// Return an exact signed significand and power of two, including subnormals.
/// Removing trailing zero bits also makes nonintegral values recognizable
/// without a floating-point comparison or a saturating integer cast.
fn exact_binary(value: f64) -> Option<(i64, i32)> {
    let bits = value.to_bits();
    let encoded_exponent = ((bits >> 52) & 0x7ff) as i32;
    if encoded_exponent == 0x7ff {
        return None;
    }
    let fraction = bits & ((1_u64 << 52) - 1);
    let (mut significand, mut exponent) = if encoded_exponent == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1_u64 << 52), encoded_exponent - 1023 - 52)
    };
    if significand == 0 {
        return Some((0, 0));
    }
    let zeros = significand.trailing_zeros();
    significand >>= zeros;
    exponent += zeros as i32;
    let signed = if bits >> 63 == 0 {
        significand as i64
    } else {
        -(significand as i64)
    };
    Some((signed, exponent))
}

fn exact_integer(value: f64) -> Option<BigInt> {
    let (significand, exponent) = exact_binary(value)?;
    if exponent < 0 {
        return None;
    }
    Some(BigInt::from(significand) << exponent as usize)
}

/// Check a bounded-variable Farkas certificate using integers only.
///
/// A compatible multiplier is >= 0 for a >= row, <= 0 for a <= row,
/// and unrestricted for an equality. The rows then imply c*x >= d, where
/// c = A^T*y and d = b^T*y. Since every variable lies in [0,1], independently
/// c*x <= sum(max(c_j,0)). A strict contradiction proves infeasibility.
///
/// Multipliers are the exact dyadic values of the supplied finite f64s,
/// rescaled by a common positive power of two. No tolerance, approximate
/// zero, or rational reconstruction participates in accepting a certificate.
fn verifies_infeasibility(n_vars: usize, constraints: &[SparseConstraint], ray: &[f64]) -> bool {
    if ray.len() != constraints.len() {
        return false;
    }
    let Some(components) = ray
        .iter()
        .map(|&value| exact_binary(value))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let common_exponent = components
        .iter()
        .filter(|&&(significand, _)| significand != 0)
        .map(|&(_, exponent)| exponent)
        .min()
        .unwrap_or(0);
    let mut combined = vec![BigInt::from(0); n_vars];
    let mut lower_bound = BigInt::from(0);
    for (row, &(significand, exponent)) in constraints.iter().zip(&components) {
        if row.cols.len() != row.vals.len()
            || matches!(row.sense, CstrSense::Le) && significand > 0
            || matches!(row.sense, CstrSense::Ge) && significand < 0
        {
            return false;
        }
        let Some(rhs) = exact_integer(row.rhs) else {
            return false;
        };
        let weight = if significand == 0 {
            BigInt::from(0)
        } else {
            BigInt::from(significand) << (exponent - common_exponent) as usize
        };
        lower_bound += &weight * rhs;
        for (&column, &coefficient) in row.cols.iter().zip(&row.vals) {
            let Ok(column) = usize::try_from(column) else {
                return false;
            };
            if column >= n_vars {
                return false;
            }
            let Some(coefficient) = exact_integer(coefficient) else {
                return false;
            };
            // Aggregate duplicate columns before bounding the linear form.
            combined[column] += &weight * coefficient;
        }
    }
    let upper_bound: BigInt = combined
        .into_iter()
        .filter(|coefficient| coefficient.sign() == Sign::Plus)
        .sum();
    lower_bound > upper_bound
}

fn certify_highs_infeasibility(
    solved: &mut SolvedModel,
    n_vars: usize,
    constraints: &[SparseConstraint],
) -> FilterResult {
    let unverified = |reason: &str| FilterResult::Undetermined {
        reason: format!(
            "HiGHS reported infeasibility without a verified exact certificate: {reason}"
        ),
    };
    if constraints.is_empty() {
        return unverified("the bounded model has no row constraints");
    }
    let pointer = solved.as_ptr();
    // SAFETY: the owned model remains alive and is exclusively borrowed for
    // this operation. Dimension queries do not write through output buffers.
    let (rows, columns) = unsafe {
        (
            highs_sys::Highs_getNumRow(pointer),
            highs_sys::Highs_getNumCol(pointer),
        )
    };
    if usize::try_from(rows).ok() != Some(constraints.len())
        || usize::try_from(columns).ok() != Some(n_vars)
    {
        return unverified("model dimensions do not match the original constraints");
    }
    let mut ray = vec![0.0; constraints.len()];
    let mut has_ray: highs_sys::HighsInt = 0;
    // SAFETY: ray has exactly the checked number of original model rows;
    // has_ray is a valid HighsInt output. HiGHS may solve the LP relaxation
    // again to obtain the ray, which is why the model is borrowed mutably.
    let status = unsafe { highs_sys::Highs_getDualRay(pointer, &mut has_ray, ray.as_mut_ptr()) };
    if status != highs_sys::STATUS_OK || has_ray != 1 {
        return unverified("no dual ray is available");
    }
    if verifies_infeasibility(n_vars, constraints, &ray) {
        FilterResult::Rejected {
            reason: "an exact integer check of a HiGHS dual ray proves the relaxation infeasible"
                .to_owned(),
        }
    } else {
        unverified("the dual ray did not pass exact verification")
    }
}

fn classify_highs_status(status: HighsModelStatus) -> io::Result<FilterResult> {
    match status {
        HighsModelStatus::Optimal
        | HighsModelStatus::ObjectiveTarget
        | HighsModelStatus::ObjectiveBound
        | HighsModelStatus::Unbounded => Ok(FilterResult::Pass),
        HighsModelStatus::Infeasible => Ok(FilterResult::Undetermined {
            reason: "HiGHS reported infeasibility without an exact certificate".to_owned(),
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
/// 双対レイを整数演算で検証できたときだけ棄却する。証明書を検証できない
/// 場合と時間上限などの未確定には理由を付ける。整数条件だけによる
/// 実行不能性も、連続緩和の証明書がなければ未確定とする。
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
    // Fdir[sq][col][dir] : sqにcolの石を置いて，dir方向にflipしたか表す論理変数
    let mut fdir: Vec<Vec<Vec<i32>>> = vec![vec![vec![-1; 8]; 2]; 64];

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
            for (dir, direction) in Direction::all().iter().enumerate() {
                let (dx, dy) = direction.to_offset();
                let mut sqs: Vec<usize> = vec![];
                let mut rl = 1;
                let mut x1 = x + dx;
                let mut y1 = y + dy;
                let mut samedir: Vec<i32> = vec![];
                while 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && (o & (1 << xy2sq(x1, y1))) != 0 {
                    rl += 1;
                    let sq1 = xy2sq(x1, y1);
                    //if y == 0 && x == 4 {
                    //    println!("x={}, y={}, sq={}, o={}, x1={}, y1={}, dir={}, rl={}", x, y, sq, o, x1, y1, dir, rl)    ;
                    //}
                    if rl >= 3 {
                        let v = vm.mk_var();
                        samedir.push(v);
                        for &sq2 in &sqs {
                            flip[sq2][col].push(v);
                        }
                        base[sq1][col].push(v);
                    }
                    sqs.push(sq1);
                    x1 += dx;
                    y1 += dy;
                }
                if samedir.len() > 0 {
                    fdir[sq][col][dir] = vm.mk_var();
                    let mut vals = vec![];
                    let mut cols = vec![];
                    for &v in &samedir {
                        cols.push(v);
                        vals.push(1.0);
                    }
                    cols.push(fdir[sq][col][dir]);
                    vals.push(-1.0);
                    constraints.push(SparseConstraint {
                        cols,
                        vals,
                        sense: CstrSense::Eq,
                        rhs: 0.0,
                    });
                    for &v in &samedir {
                        let cols = vec![v, fdir[sq][col][dir]];
                        let vals = vec![1.0, -1.0];
                        constraints.push(SparseConstraint {
                            cols,
                            vals,
                            sense: CstrSense::Le,
                            rhs: 0.0,
                        });
                    }
                }
            }
            let mut vals = vec![];
            let mut cols = vec![];
            for dir in 0..8 {
                let v = fdir[sq][col][dir];
                if v >= 0 {
                    cols.push(v);
                    vals.push(1.0);
                }
            }
            cols.push(first[sq][col]);
            vals.push(-1.0);
            constraints.push(SparseConstraint {
                cols,
                vals,
                sense: CstrSense::Ge,
                rhs: 0.0,
            });
            for dir in 0..8 {
                let v = fdir[sq][col][dir];
                if v >= 0 {
                    let cols = vec![v, first[sq][col]];
                    let vals = vec![1.0, -1.0];
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
    // for all square
    let mut b = occupied;
    while b != 0 {
        let sq = b.trailing_zeros() as usize; // 0..=63
        b &= b - 1;
        // flip count constraints
        let col = if player & (1 << sq) != 0 { 0 } else { 1 };
        let mut cols = vec![first[sq][col], first[sq][1 - col]];
        let mut vals = vec![1.0, -1.0];
        for &v in &flip[sq][col] {
            cols.push(v);
            vals.push(2.0);
        }
        for &v in &flip[sq][1 - col] {
            cols.push(v);
            vals.push(-2.0);
        }
        constraints.push(SparseConstraint {
            cols,
            vals,
            sense: CstrSense::Eq,
            rhs: 1.0,
        });
        // base constriants
        for col in 0..2 {
            for &v in &base[sq][col] {
                //eprintln!("base[{}][{}]={}", sq, col, v);
                let mut cols = vec![v, first[sq][col]];
                let mut vals = vec![-1.0, 1.0];
                for &v1 in &flip[sq][col] {
                    cols.push(v1);
                    vals.push(1.0);
                }
                constraints.push(SparseConstraint {
                    cols,
                    vals,
                    sense: CstrSense::Ge,
                    rhs: 0.0,
                });
            }
        }
    }
    check_feasibility(vm.count, &constraints, by_ip_solver)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cols: &[i32], vals: &[f64], sense: CstrSense, rhs: f64) -> SparseConstraint {
        SparseConstraint {
            cols: cols.to_vec(),
            vals: vals.to_vec(),
            sense,
            rhs,
        }
    }

    #[test]
    fn binary_conversion_preserves_subnormals_signs_and_large_integers() {
        for value in [0.0, -0.0] {
            assert_eq!(exact_binary(value), Some((0, 0)));
            assert_eq!(exact_integer(value), Some(BigInt::from(0)));
        }
        assert_eq!(exact_binary(1.5), Some((3, -1)));
        assert_eq!(exact_binary(-1.5), Some((-3, -1)));
        assert_eq!(exact_binary(f64::from_bits(1)), Some((1, -1074)));
        assert_eq!(exact_binary(-f64::from_bits(1)), Some((-1, -1074)));
        assert_eq!(exact_binary(f64::MIN_POSITIVE), Some((1, -1022)));
        assert_eq!(exact_integer(1.5), None);
        assert_eq!(exact_integer(f64::from_bits(1)), None);
        assert_eq!(
            exact_integer(2_f64.powi(63)),
            Some(BigInt::from(1) << 63_usize)
        );
        assert_eq!(
            exact_integer(-2_f64.powi(63)),
            Some(-(BigInt::from(1) << 63_usize))
        );
        assert_eq!(
            exact_integer(f64::MAX),
            Some(BigInt::from((1_u64 << 53) - 1) << 971_usize),
        );
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(exact_binary(value), None);
            assert_eq!(exact_integer(value), None);
        }
    }

    #[test]
    fn exact_certificates_handle_inequalities_equalities_and_box_bounds() {
        let contradictory = [
            row(&[0], &[1.0], CstrSense::Ge, 1.0),
            row(&[0], &[1.0], CstrSense::Le, 0.0),
        ];
        for scale in [f64::from_bits(1), 0.125, 1.0, 2_f64.powi(900), f64::MAX] {
            assert!(verifies_infeasibility(1, &contradictory, &[scale, -scale]));
        }
        // Cancellation need not be exact: box bounds absorb a residual c.
        assert!(verifies_infeasibility(1, &contradictory, &[0.5, -0.25]));
        assert!(verifies_infeasibility(
            1,
            &[
                row(&[0], &[1.0], CstrSense::Eq, 0.0),
                row(&[0], &[1.0], CstrSense::Eq, 1.0),
            ],
            &[-0.75, 0.75],
        ));
        assert!(verifies_infeasibility(
            1,
            &[row(&[0], &[1.0], CstrSense::Ge, 2.0)],
            &[1.0],
        ));
        assert!(verifies_infeasibility(
            1,
            &[row(&[0], &[1.0], CstrSense::Le, -1.0)],
            &[-1.0],
        ));
        assert!(verifies_infeasibility(
            0,
            &[row(&[], &[], CstrSense::Eq, 1.0)],
            &[1.0],
        ));
        assert!(verifies_infeasibility(
            1,
            &[row(&[0, 0], &[2.0, -1.0], CstrSense::Ge, 2.0)],
            &[1.0],
        ));
    }

    #[test]
    fn feasible_boxes_and_wrong_signs_never_certify_infeasibility() {
        // (x,y)=(0,1) satisfies every row. Exhaust compatible signed rays.
        let feasible = [
            row(&[0, 1], &[1.0, 1.0], CstrSense::Eq, 1.0),
            row(&[0], &[1.0], CstrSense::Ge, 0.0),
            row(&[1], &[1.0], CstrSense::Le, 1.0),
        ];
        for equality in -3..=3 {
            for lower in 0..=3 {
                for upper in -3..=0 {
                    assert!(!verifies_infeasibility(
                        2,
                        &feasible,
                        &[equality as f64, lower as f64, upper as f64],
                    ));
                }
            }
        }
        assert!(!verifies_infeasibility(
            2,
            &feasible,
            &[f64::MAX, f64::from_bits(1), -f64::MAX],
        ));
        // Equality at the box maximum is feasible; the comparison is strict.
        assert!(!verifies_infeasibility(
            1,
            &[row(&[0], &[1.0], CstrSense::Ge, 1.0)],
            &[1.0],
        ));
        // These multipliers would give false contradictions if row senses
        // were ignored, although each single-row problem is feasible.
        assert!(!verifies_infeasibility(
            1,
            &[row(&[0], &[1.0], CstrSense::Le, 2.0)],
            &[1.0],
        ));
        assert!(!verifies_infeasibility(
            1,
            &[row(&[0], &[1.0], CstrSense::Ge, -1.0)],
            &[-1.0],
        ));
        assert!(!verifies_infeasibility(1, &[], &[]));
    }

    #[test]
    fn malformed_or_nonintegral_certificate_inputs_fail_closed() {
        let impossible = [row(&[0], &[1.0], CstrSense::Ge, 2.0)];
        for ray in [vec![], vec![1.0, 1.0], vec![f64::NAN], vec![f64::INFINITY]] {
            assert!(!verifies_infeasibility(1, &impossible, &ray));
        }
        for invalid in [
            row(&[0], &[], CstrSense::Ge, 2.0),
            row(&[], &[1.0], CstrSense::Ge, 2.0),
            row(&[-1], &[1.0], CstrSense::Ge, 2.0),
            row(&[1], &[1.0], CstrSense::Ge, 2.0),
            row(&[0], &[0.5], CstrSense::Ge, 2.0),
            row(&[0], &[f64::NAN], CstrSense::Ge, 2.0),
            row(&[0], &[f64::INFINITY], CstrSense::Ge, 2.0),
            row(&[0], &[1.0], CstrSense::Ge, 1.5),
            row(&[0], &[1.0], CstrSense::Ge, f64::NAN),
            row(&[0], &[1.0], CstrSense::Ge, f64::INFINITY),
        ] {
            assert!(!verifies_infeasibility(1, &[invalid], &[1.0]));
        }
        // Even zero-weight rows must have valid model data.
        assert!(!verifies_infeasibility(
            1,
            &[
                row(&[0], &[1.0], CstrSense::Ge, 2.0),
                row(&[0], &[0.5], CstrSense::Eq, 0.0),
            ],
            &[1.0, 0.0],
        ));
    }

    #[test]
    fn highs_rejection_requires_an_actual_exact_certificate() {
        let contradictory = [
            row(&[0], &[1.0], CstrSense::Ge, 1.0),
            row(&[0], &[1.0], CstrSense::Le, 0.0),
        ];
        for integral in [false, true] {
            let outcome = check_feasibility(1, &contradictory, integral).unwrap();
            assert!(
                matches!(outcome, FilterResult::Rejected { .. }),
                "{outcome:?}"
            );
            assert!(outcome.reason().contains("exact integer check"));
        }
        // Integer infeasibility alone has no continuous-relaxation ray:
        // 2*x=1 admits x=1/2 in [0,1] but no binary x.
        let only_integer_infeasible = [row(&[0], &[2.0], CstrSense::Eq, 1.0)];
        assert!(matches!(
            check_feasibility(1, &only_integer_infeasible, false).unwrap(),
            FilterResult::Pass,
        ));
        assert!(matches!(
            check_feasibility(1, &only_integer_infeasible, true).unwrap(),
            FilterResult::Undetermined { .. },
        ));
    }

    #[test]
    fn a_highs_status_without_a_certificate_never_rejects() {
        for status in [
            HighsModelStatus::Infeasible,
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
