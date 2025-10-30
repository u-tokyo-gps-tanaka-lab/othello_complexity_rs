#[allow(unused_imports)]
use crate::othello::{Board, CENTER_MASK, DXYS};
use crate::prunings::check_occupancy::occupancy_order;
use highs::{HighsModelStatus, RowProblem, Sense};
use std::ffi::CString;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

#[allow(dead_code)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeasResult {
    Feasible,
    Infeasible,
    Unknown,
}

pub struct VarMaker {
    count: usize,
    symbols: Vec<String>,
}
// no var isize: -1
impl VarMaker {
    pub fn new() -> Self {
        VarMaker {
            count: 0,
            symbols: vec![],
        }
    }
    fn mk_var(&mut self, sym: String) -> i32 {
        let ans = self.count;
        self.symbols.push(sym);
        self.count += 1;
        ans as i32
    }
    fn count(&self) -> i32 {
        self.count as i32
    }
    fn get_symbol(&self, i: usize) -> String {
        self.symbols[i].to_string()
    }
}

#[inline]
fn xy2sq(x: i32, y: i32) -> usize {
    (y * 8 + x) as usize
}
/// 解の列変数値をファイルに書き出す。
/// - `round_binary`: 0/1 変数を 0/1 に丸めて出力したいとき true（許容誤差は 1e-9）
/// 出力形式: `<col_index>\t<value>\n`
pub fn dump_solution_columns<P: AsRef<Path>>(
    solved: &highs::SolvedModel, // model.solve() の返り値への参照
    out_path: P,
    round_binary: bool,
    vm: &VarMaker,
) -> io::Result<()> {
    // ステータス確認（必要に応じて緩めてもよい）
    match solved.status() {
        HighsModelStatus::Optimal
        | HighsModelStatus::ObjectiveTarget
        | HighsModelStatus::ObjectiveBound
        | HighsModelStatus::Unbounded => {} // 可行解がある
        st => eprintln!("warning: model status = {:?}", st),
    }

    let sol = solved.get_solution();
    let cols = sol.columns(); // 変数値（f64）のスライス

    let f = File::create(out_path)?;
    let mut w = BufWriter::new(f);

    for (i, &x) in cols.iter().enumerate() {
        let v = if round_binary {
            // 0/1 を想定した丸め（適宜調整）
            if (x - 0.0).abs() < 1e-9 {
                0.0
            } else if (x - 1.0).abs() < 1e-9 {
                1.0
            } else {
                x
            } // 大きく外れていたらそのまま出す／あるいはエラーにする
        } else {
            x
        };
        writeln!(w, "{}\t{}", vm.get_symbol(i), v)?;
    }
    w.flush()?;
    Ok(())
}

/// 連続緩和(0<=x<=1)で可否のみ判定 (HiGHS 1.12.0 API)
fn check_feasibility(
    n_vars: usize,
    constraints: &[SparseConstraint],
    by_ip_solver: bool,
    vm: &VarMaker,
) -> FeasResult {
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

    // モデル化 → オプション設定 → 解く
    let mut model = pb.optimise(Sense::Minimise);
    unsafe {
        // モデル名
        let mptr = model.as_mut_ptr();
        // 列（変数）名：生成順の index を使う
        for i in 0..n_vars {
            let col_name = vm.get_symbol(i);
            highs_sys::Highs_passColName(mptr, i as i32, CString::new(col_name).unwrap().as_ptr());
        }
    }
    //
    //model.set_option("output_flag", true);          // ログ表示
    //model.set_option("log_dev_level", 1);
    //model.set_option("write_model_file", "debug.lp");          // .lp か .mps
    //model.set_option("write_model_to_file", true);
    model.set_option("threads", 1i32);
    // “可否が分かれば十分”向けの軽量設定（任意）
    //if !by_ip_solver {
    //    let _ = model.set_option("solver", "ipm");         // IPMを使う
    //    let _ = model.set_option("run_crossover", "off");  // クロスオーバー無効
    //  let _ = model.set_option("presolve", "on");        // presolve 明示
    //}
    // let _ = model.set_option("threads", 4);         // 並列数を指定したい場合
    // let _ = model.set_option("time_limit", 5.0);    // 早期打切り

    let solved = model.solve(); // v1.12の標準手順  [oai_citation:1‡docs.rs](https://docs.rs/highs/latest/highs/struct.Model.html)
                                //dump_solution_columns(&solved, "vars.tsv", /*round_binary=*/true, &vm);
                                // ステータスを可否に丸める
    match solved.status() {
        // 実行可能（最適・目標到達・下界到達・非有界は可行点が存在）
        HighsModelStatus::Optimal
        | HighsModelStatus::ObjectiveTarget
        | HighsModelStatus::ObjectiveBound
        | HighsModelStatus::Unbounded => FeasResult::Feasible,

        // 非実行可能（または非有界/非実行可能の二者不定）
        HighsModelStatus::Infeasible | HighsModelStatus::UnboundedOrInfeasible => {
            FeasResult::Infeasible
        }

        // それ以外は不明（時間制限・反復上限・ロード/ソルブエラー等）
        _ => FeasResult::Unknown,
    }
}

pub fn check_lp(player: u64, opponent: u64, by_ip_solver: bool) -> bool {
    //let b = Board::new(player, opponent);
    //println!("b={}", b.to_string());
    let occupied = player | opponent;
    let order: [u64; 64] = occupancy_order(occupied);
    let mut vm = VarMaker::new();
    let mut constraints = vec![];
    // First[sq][col] : sqにcolの石を置いたかを表す論理変数
    let mut first: Vec<Vec<i32>> = vec![vec![-1; 2]; 64];
    // center
    first[3 * 8 + 3][0] = vm.mk_var(format!("first_{}_{}", 3 * 8 + 3, 0));
    first[3 * 8 + 3][1] = vm.mk_var(format!("first_{}_{}", 3 * 8 + 3, 1));
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
        first[sq][0] = vm.mk_var(format!("first_{}_{}", sq, 0));
        first[sq][1] = vm.mk_var(format!("first_{}_{}", sq, 1));
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
            for (dir, (dx, dy)) in DXYS.iter().enumerate() {
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
                        let v = vm.mk_var(format!("f_{}_{}_{}_{}", sq, col, dir, rl));
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
                    fdir[sq][col][dir] = vm.mk_var(format!("fdir_{}_{}_{}", sq, col, dir));
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
    let n = vm.count() as usize;
    let res = check_feasibility(n, &constraints, by_ip_solver, &vm);
    //println!("Feasibility (continuous relaxation): {:?}", res);
    if res == FeasResult::Infeasible {
        false
    } else {
        true
    }
}
