use crate::io::{TriCategory, TriOutcome};
use crate::othello::{board_with_symmetry, Board};
use rayon::prelude::*;
use rustsat::{
    instances::Cnf,
    solvers::{Interrupt, InterruptSolver, Solve, SolverResult},
    types::{Clause, Lit, TernaryVal},
};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

const BOARD_SQUARES: usize = 64;
const DIRS: [(i32, i32); 8] = [
    (0, 1),   // N
    (1, 1),   // NE
    (1, 0),   // E
    (1, -1),  // SE
    (0, -1),  // S
    (-1, -1), // SW
    (-1, 0),  // W
    (-1, 1),  // NW
];

pub const INITIAL_BLACK: u64 = 0x0000_0008_1000_0000;
pub const INITIAL_WHITE: u64 = 0x0000_0010_0800_0000;
pub const INITIAL_BOARD: Board = Board {
    player: INITIAL_BLACK,
    opponent: INITIAL_WHITE,
};

#[derive(Clone, Copy, Debug)]
// 「play_sq に着手し、dir 方向で相手石を k 枚はさむ」を表す
struct FlipSource {
    play_sq: usize, // 着手マスの添字 (範囲は 0..BOARD_SQUARES)
    dir: usize,     // 方向添字 (範囲は 0..DIRS.len())
    k: usize,       // dir 方向で連続して挟む相手石の枚数 (範囲は 1..=rays[play_sq][dir].len()-1)
}

// rays[sq][dir]: sq から dir 方向へ進んだマスの一覧
// sq の範囲は 0..BOARD_SQUARES、dir の範囲は 0..DIRS.len()
type Rays = Vec<Vec<Vec<usize>>>;

// flip_sources[target_sq]: target_sq を反転させ得る FlipSource の一覧
// target_sq の範囲は 0..BOARD_SQUARES
type FlipSources = Vec<Vec<FlipSource>>;

#[derive(Default)]
struct ClauseBuilder {
    clauses: Vec<Vec<i32>>,
}

impl ClauseBuilder {
    fn add_clause(&mut self, clause: Vec<i32>) {
        self.clauses.push(clause);
    }

    fn add_unit(&mut self, lit: i32) {
        self.clauses.push(vec![lit]);
    }

    fn add_implication(&mut self, antecedent: i32, consequent: i32) {
        self.clauses.push(vec![-antecedent, consequent]);
    }

    fn add_implication_many(&mut self, antecedents: &[i32], consequent: i32) {
        let mut clause = Vec::with_capacity(antecedents.len() + 1);
        for &lit in antecedents {
            clause.push(-lit);
        }
        clause.push(consequent);
        self.clauses.push(clause);
    }

    fn into_clauses(self) -> Vec<Vec<i32>> {
        self.clauses
    }
}

/// layer - 盤面の状態添字, 範囲は 0..=h（総数は h+1）
/// ply - 1手ごとの状態遷移の添字, 範囲は 0..h（総数は h）
struct SatVars {
    h: usize,                     // 所要手数 (plyの総数)
    var_count: i32,               // これまでに割り当てた DIMACS 変数番号の最大値
    b: Vec<Vec<i32>>,             // b[layer][sq]: マス sq は layer 0..=h で黒石になっている
    w: Vec<Vec<i32>>,             // w[layer][sq]: マス sq は layer 0..=h で白石になっている
    turn: Vec<i32>,               // turn[layer]: layer における手番 (true = black, false = white).
    pass: Vec<i32>,               // pass[ply]: ply がパス手か否か
    play: Vec<Vec<i32>>,          // play[ply][sq]: ply で sq に着手する
    legal: Vec<Vec<i32>>,         // legal[ply][sq]: sq が ply の合法手である
    flip: Vec<Vec<i32>>,          // flip[ply][sq]: sq が ply で反転する
    cap: Vec<Vec<Vec<Vec<i32>>>>, // cap[ply][sq][dir][k]: sq への着手が方向 dir で k 枚を挟む
    cause: Vec<Vec<Vec<i32>>>,    // cause[ply][sq][i]: ply 手目で sq が反転する原因 i が成立する
}

impl SatVars {
    fn new(h: usize, rays: &Rays, flip_sources: &FlipSources) -> Self {
        let mut next = 0_i32;

        let b = alloc_matrix(&mut next, h + 1, BOARD_SQUARES);
        let w = alloc_matrix(&mut next, h + 1, BOARD_SQUARES);
        let turn = alloc_vector(&mut next, h + 1);
        let pass = alloc_vector(&mut next, h);
        let play = alloc_matrix(&mut next, h, BOARD_SQUARES);
        let legal = alloc_matrix(&mut next, h, BOARD_SQUARES);
        let flip = alloc_matrix(&mut next, h, BOARD_SQUARES);

        let mut cap: Vec<Vec<Vec<Vec<i32>>>> =
            vec![vec![vec![Vec::new(); DIRS.len()]; BOARD_SQUARES]; h];
        for ply in 0..h {
            for s in 0..BOARD_SQUARES {
                for d in 0..DIRS.len() {
                    let max_k = rays[s][d].len().saturating_sub(1);
                    // k = 0 is a dummy slot. Real capture variables are k >= 1 only.
                    let mut ks = vec![0_i32; max_k + 1];
                    for slot in ks.iter_mut().take(max_k + 1).skip(1) {
                        next += 1;
                        *slot = next;
                    }
                    cap[ply][s][d] = ks;
                }
            }
        }

        let mut cause: Vec<Vec<Vec<i32>>> = vec![vec![Vec::new(); BOARD_SQUARES]; h];
        for ply in 0..h {
            for (s, slot) in cause[ply].iter_mut().enumerate().take(BOARD_SQUARES) {
                // One variable per precomputed source in flip_sources[s].
                let mut vars = vec![0_i32; flip_sources[s].len()];
                for var in &mut vars {
                    next += 1;
                    *var = next;
                }
                *slot = vars;
            }
        }

        Self {
            h,
            var_count: next,
            b,
            w,
            turn,
            pass,
            play,
            legal,
            flip,
            cap,
            cause,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum PlyAction {
    Pass,
    Play(usize),
}

#[derive(Clone, Copy, Debug)]
struct Ply {
    turn_black: bool,
    action: PlyAction,
}

#[derive(Debug)]
struct SatWitness {
    h: usize,
    plies: Vec<Ply>,
    boards: Vec<Board>,
}

pub struct RunOptions {
    pub start: Board,
    pub goals: Vec<Board>,
    pub check_symmetry: bool,
    pub start_turn_black: bool,
    pub parallel_goals: usize,
    pub show_coords: bool,
    pub show_boards: bool,
    pub verbose: bool,
    pub cnf_dump_dir: Option<PathBuf>,
    pub cnf_dump_only: bool,
    pub sat_timeout_per_depth: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoalClassification {
    Reachable { goal: Board },
    Unreachable { goal: Board },
    Unknown { goal: Board },
}

impl GoalClassification {
    pub fn goal(&self) -> Board {
        match *self {
            Self::Reachable { goal } | Self::Unreachable { goal } | Self::Unknown { goal } => goal,
        }
    }
}

impl TriOutcome for GoalClassification {
    fn category(&self) -> TriCategory {
        match self {
            GoalClassification::Reachable { .. } => TriCategory::Ok,
            GoalClassification::Unreachable { .. } => TriCategory::Ng,
            GoalClassification::Unknown { .. } => TriCategory::Unknown,
        }
    }
}

enum GoalSolveResult {
    Sat(SatWitness),
    Unsat,
    Unknown,
}

pub fn run_with_options<F>(
    opts: RunOptions,
    mut on_outcome: F,
) -> Result<Vec<GoalClassification>, String>
where
    F: FnMut(&GoalClassification) -> Result<(), String>,
{
    let RunOptions {
        start,
        goals,
        check_symmetry,
        start_turn_black,
        parallel_goals,
        show_coords,
        show_boards,
        verbose,
        cnf_dump_dir,
        cnf_dump_only,
        sat_timeout_per_depth,
    } = opts;

    if let Some(dir) = &cnf_dump_dir {
        fs::create_dir_all(dir).map_err(|e| {
            format!(
                "failed to create CNF output directory {}: {}",
                dir.display(),
                e
            )
        })?;
    }

    let rays = precompute_rays();
    let flip_sources = precompute_flip_sources(&rays);
    let many_goals = goals.len() > 1;
    let mut outcomes: Vec<GoalClassification> = Vec::with_capacity(goals.len());
    let goals_with_index: Vec<(usize, Board)> = goals
        .into_iter()
        .enumerate()
        .map(|(idx, goal)| (idx + 1, goal))
        .collect();
    let chunk_size = parallel_goals.max(1);

    for chunk in goals_with_index.chunks(chunk_size) {
        if many_goals {
            for (goal_index, goal) in chunk {
                println!("goal[{}] {}", goal_index, goal.to_string());
            }
        } else {
            println!("goal {}", chunk[0].1.to_string())
        }

        let solved: Vec<Result<(usize, Board, GoalSolveResult), String>> = chunk
            .par_iter()
            .map(|(goal_index, goal)| {
                let sat_result = check_sat_reachability(
                    start,
                    *goal,
                    *goal_index,
                    check_symmetry,
                    start_turn_black,
                    verbose,
                    cnf_dump_dir.as_deref(),
                    cnf_dump_only,
                    sat_timeout_per_depth,
                    &rays,
                    &flip_sources,
                )?;
                Ok((*goal_index, *goal, sat_result))
            })
            .collect();

        for result in solved {
            let (goal_index, goal, sat_result) = result?;

            if cnf_dump_only {
                if verbose {
                    println!("[info] CNF dump only completed for goal[{}]", goal_index);
                }
                continue;
            }

            match sat_result {
                GoalSolveResult::Sat(witness) => {
                    print_sat_result(&witness, goal_index, show_coords, show_boards);
                    let outcome = GoalClassification::Reachable { goal };
                    outcomes.push(outcome);
                    on_outcome(&outcome)?;
                }
                GoalSolveResult::Unsat => {
                    println!("UNSAT");
                    let outcome = GoalClassification::Unreachable { goal };
                    outcomes.push(outcome);
                    on_outcome(&outcome)?;
                }
                GoalSolveResult::Unknown => {
                    println!("UNKNOWN");
                    let outcome = GoalClassification::Unknown { goal };
                    outcomes.push(outcome);
                    on_outcome(&outcome)?;
                }
            }
        }
    }
    Ok(outcomes)
}

fn check_sat_reachability(
    start: Board,
    goal: Board,
    goal_index: usize,
    check_symmetry: bool,
    start_turn_black: bool,
    verbose: bool,
    cnf_dump_dir: Option<&Path>,
    cnf_dump_only: bool,
    sat_timeout_per_depth: Option<Duration>,
    rays: &Rays,
    flip_sources: &FlipSources,
) -> Result<GoalSolveResult, String> {
    let (goals_to_solve, goal_symmetry_count) = if check_symmetry {
        all_goals_to_solve(start, goal)
    } else {
        (vec![(0, goal)], 1)
    };
    let min_plies = match min_required_plies(start, goal) {
        Some(v) => v,
        None => {
            if verbose {
                println!(
                    "[info][goal={}] unreachable by disc count: start_discs={} goal_discs={}",
                    goal_index,
                    start.popcount(),
                    goal.popcount()
                );
            }
            return Ok(GoalSolveResult::Unsat);
        }
    };
    let max_plies = max_plies_with_passes(min_plies);

    if verbose {
        println!(
                "[info][goal={}] start iterative deepening from min_plies={} to max_plies={} (start_discs={}, goal_discs={})",
                goal_index,
                min_plies,
                max_plies,
                start.popcount(),
                goal.popcount()
            );
        if check_symmetry {
            println!(
                "[info][goal={}] this goal has {} symmetric candidate(s) (from all {} symmetries)",
                goal_index,
                goals_to_solve.len(),
                goal_symmetry_count,
            );
        } else {
            println!(
                "[info][goal={}] symmetry check is disabled; solving only the original goal",
                goal_index
            );
        }
    }

    let mut is_unknown = false;
    for (goal_symmetry, goal_board) in goals_to_solve {
        if verbose && goal_symmetry != 0 {
            println!(
                "[info][goal={}] trying goal symmetry={}",
                goal_index, goal_symmetry
            );
        }
        for h in min_plies..=max_plies {
            let vars = SatVars::new(h, rays, flip_sources);
            let clauses = encode_problem(
                &vars,
                start,
                goal_board,
                start_turn_black,
                rays,
                flip_sources,
            );
            dump_dimacs_cnf(
                cnf_dump_dir,
                goal_index,
                goal_symmetry,
                h,
                vars.var_count as usize,
                &clauses,
            )?;

            if verbose {
                println!(
                    "[info][goal={}] depth={} symmetry={} vars={} clauses={}",
                    goal_index,
                    h,
                    goal_symmetry,
                    vars.var_count,
                    clauses.len()
                );
            }

            if cnf_dump_only {
                continue;
            }

            match solve_single_problem(&vars, &clauses, sat_timeout_per_depth)? {
                GoalSolveResult::Sat(witness) => {
                    return Ok(GoalSolveResult::Sat(witness));
                }
                GoalSolveResult::Unsat => {}
                GoalSolveResult::Unknown => {
                    is_unknown = true;
                    break;
                }
            }
        }
    }
    if is_unknown {
        Ok(GoalSolveResult::Unknown)
    } else {
        Ok(GoalSolveResult::Unsat)
    }
}

fn dump_dimacs_cnf(
    cnf_dump_dir: Option<&Path>,
    goal_index: usize,
    goal_symmetry: i32,
    h: usize,
    var_count: usize,
    clauses: &[Vec<i32>],
) -> Result<(), String> {
    let Some(base_dir) = cnf_dump_dir else {
        return Ok(());
    };
    let filename = format!(
        "goal_{:04}_sym_{}_h_{:03}.cnf",
        goal_index, goal_symmetry, h
    );
    let path = base_dir.join(filename);

    let file = File::create(&path)
        .map_err(|e| format!("failed to create CNF file {}: {}", path.display(), e))?;
    let mut writer = BufWriter::new(file);

    writeln!(writer, "p cnf {} {}", var_count, clauses.len())
        .map_err(|e| format!("failed to write DIMACS header {}: {}", path.display(), e))?;
    for clause in clauses {
        for lit in clause {
            write!(writer, "{} ", lit)
                .map_err(|e| format!("failed to write DIMACS clause {}: {}", path.display(), e))?;
        }
        writeln!(writer, "0")
            .map_err(|e| format!("failed to finalize DIMACS clause {}: {}", path.display(), e))?;
    }
    writer
        .flush()
        .map_err(|e| format!("failed to flush CNF file {}: {}", path.display(), e))?;
    Ok(())
}

fn min_required_plies(start: Board, goal: Board) -> Option<usize> {
    let start_discs = start.popcount();
    let goal_discs = goal.popcount();
    if goal_discs < start_discs {
        return None;
    }
    Some((goal_discs - start_discs) as usize)
}

fn max_plies_with_passes(min_plies: usize) -> usize {
    // Each non-pass move can be preceded by at most one forced pass.
    min_plies.saturating_mul(2)
}

/// 対称性を考慮し, かつ正規化を行ったgoalを計算する
fn all_goals_to_solve(start: Board, goal: Board) -> (Vec<(i32, Board)>, usize) {
    // goalと対称な最大8通りの盤面を列挙
    let goal_symmtries = {
        let mut out = Vec::with_capacity(8);
        for sym in 0_i32..8_i32 {
            let transformed = board_with_symmetry(goal, sym);
            if out.iter().all(|(_, existing)| *existing != transformed) {
                out.push((sym, transformed));
            }
        }
        out
    };

    // s(start) = start となる対称変換 s を列挙
    // 任意の対称変換 s について IsReachable(start, goal) と IsReachable(s(start), s(goal)) は同値なので,
    // s(start) = start ならば s(goal) と goal を同一視して良い.
    let start_invariants = {
        let mut out = Vec::with_capacity(8);
        for sym in 0_i32..8_i32 {
            if board_with_symmetry(start, sym) == start {
                out.push(sym);
            }
        }
        out
    };

    let mut selected = Vec::new();
    let mut all_canonicals = HashSet::new();

    for &(g_sym, b) in &goal_symmtries {
        let mut canonical = b;
        for &inv in &start_invariants {
            let tmp_canonical = board_with_symmetry(goal, inv);
            if tmp_canonical < canonical {
                canonical = tmp_canonical;
            }
        }
        if all_canonicals.insert(canonical) {
            selected.push((g_sym, b));
        }
    }

    (selected, goal_symmtries.len())
}

fn encode_problem(
    vars: &SatVars,
    start: Board,
    goal: Board,
    start_turn_black: bool,
    rays: &Rays,
    flip_sources: &FlipSources,
) -> Vec<Vec<i32>> {
    let mut builder = ClauseBuilder::default();

    // (1) 開始層と終端層の石配置を固定し、同一マスの黒白重複を禁止する
    // b[0][s]=start.player[s], w[0][s]=start.opponent[s]
    // b[h][s]=goal.player[s], w[h][s]=goal.opponent[s]
    // forall layer,sq: not(b[layer][sq] and w[layer][sq])
    for s in 0..BOARD_SQUARES {
        builder.add_unit(if bit_is_set(start.player, s) {
            vars.b[0][s]
        } else {
            -vars.b[0][s]
        });
        builder.add_unit(if bit_is_set(start.opponent, s) {
            vars.w[0][s]
        } else {
            -vars.w[0][s]
        });
        builder.add_unit(if bit_is_set(goal.player, s) {
            vars.b[vars.h][s]
        } else {
            -vars.b[vars.h][s]
        });
        builder.add_unit(if bit_is_set(goal.opponent, s) {
            vars.w[vars.h][s]
        } else {
            -vars.w[vars.h][s]
        });
    }
    for layer in 0..=vars.h {
        for s in 0..BOARD_SQUARES {
            builder.add_clause(vec![-vars.b[layer][s], -vars.w[layer][s]]);
        }
    }

    // (2) 初期手番を固定し、各層で手番が必ず反転することを要求する
    // turn[0] = start_turn_black かつ
    // forall layer<h: turn[layer+1] <-> not turn[layer]
    builder.add_unit(if start_turn_black {
        vars.turn[0]
    } else {
        -vars.turn[0]
    });
    for layer in 0..vars.h {
        // turn[layer+1] と turn[layer] が否定関係にあることを表す
        builder.add_clause(vec![vars.turn[layer + 1], vars.turn[layer]]);
        builder.add_clause(vec![-vars.turn[layer + 1], -vars.turn[layer]]);
    }

    for ply in 0..vars.h {
        let layer = ply;
        let next_layer = layer + 1;
        let pass_ply = vars.pass[ply];
        let turn_layer = vars.turn[layer];

        // (3) 各手で「パス」または「1マス着手」のいずれかのみを許可する
        // forall ply: pass[ply] or (OR_sq play[ply][sq])
        // forall ply,sq: pass[ply] -> not play[ply][sq]
        // forall ply, i!=j: (not pass[ply]) -> not(play[ply][i] and play[ply][j])
        let mut at_least_one_play = Vec::with_capacity(BOARD_SQUARES + 1);
        at_least_one_play.push(pass_ply); // !pass[ply] のときにどこか1マスへの着手を必須にする
        for &play_sq in &vars.play[ply] {
            at_least_one_play.push(play_sq);
            builder.add_clause(vec![-pass_ply, -play_sq]); // pass[ply] が真なら play[ply][sq] を偽にする
        }
        builder.add_clause(at_least_one_play);
        for i in 1..BOARD_SQUARES {
            for j in 0..i {
                builder.add_clause(vec![pass_ply, -vars.play[ply][i], -vars.play[ply][j]]);
            }
        }

        // (4) 合法手変数と方向別捕獲変数の意味を定義する
        // legal[ply][sq] -> empty(sq) と cap[ply][sq][dir][k] -> legal[ply][sq]
        // 1. turn[layer] の色ごとに cap[ply][sq][dir][k] と挟み込み条件 (ray上に相手石 k 枚が続き、その先に自石が1枚ある) を対応付ける
        // 2. legal[ply][sq] -> OR_{dir,k} cap[ply][sq][dir][k] を課し、候補が存在しないときは legal[ply][sq] を偽に固定する
        for sq in 0..BOARD_SQUARES {
            let legal_ply_sq = vars.legal[ply][sq];
            let b_layer_sq = vars.b[layer][sq];
            let w_layer_sq = vars.w[layer][sq];
            builder.add_clause(vec![-legal_ply_sq, -b_layer_sq]); // legal[ply][sq] が真なら sq に黒石がない
            builder.add_clause(vec![-legal_ply_sq, -w_layer_sq]); // legal[ply][sq] が真なら sq に白石がない

            let mut cap_vars = Vec::new();
            for dir in 0..DIRS.len() {
                let ray = &rays[sq][dir];
                let max_k = ray.len().saturating_sub(1);
                for k in 1..=max_k {
                    let cap_ply_sq_dir_k = vars.cap[ply][sq][dir][k];
                    cap_vars.push(cap_ply_sq_dir_k);

                    // cap[ply][sq][dir][k] が真なら legal[ply][sq] も真である
                    builder.add_implication(cap_ply_sq_dir_k, legal_ply_sq);

                    // turn[layer]=black で黒側の挟み込み条件が成立したときに cap[ply][sq][dir][k] を真にする
                    let mut ants_black = Vec::with_capacity(k + 4);
                    ants_black.push(turn_layer);
                    ants_black.push(-b_layer_sq);
                    ants_black.push(-w_layer_sq);
                    for idx in ray.iter().take(k) {
                        ants_black.push(vars.w[layer][*idx]);
                    }
                    ants_black.push(vars.b[layer][ray[k]]);
                    builder.add_implication_many(&ants_black, cap_ply_sq_dir_k);

                    // turn[layer]=white で白側の挟み込み条件が成立したときに cap[ply][sq][dir][k] を真にする
                    let mut ants_white = Vec::with_capacity(k + 4);
                    ants_white.push(-turn_layer);
                    ants_white.push(-b_layer_sq);
                    ants_white.push(-w_layer_sq);
                    for idx in ray.iter().take(k) {
                        ants_white.push(vars.b[layer][*idx]);
                    }
                    ants_white.push(vars.w[layer][ray[k]]);
                    builder.add_implication_many(&ants_white, cap_ply_sq_dir_k);

                    // cap[ply][sq][dir][k] と turn[layer]=black が真なら黒側の挟み込み条件を強制する
                    for cond in ants_black.iter().skip(1) {
                        builder.add_implication_many(&[cap_ply_sq_dir_k, turn_layer], *cond);
                    }
                    // cap[ply][sq][dir][k] と turn[layer]=white が真なら白側の挟み込み条件を強制する
                    for cond in ants_white.iter().skip(1) {
                        builder.add_implication_many(&[cap_ply_sq_dir_k, -turn_layer], *cond);
                    }
                }
            }

            if cap_vars.is_empty() {
                builder.add_unit(-legal_ply_sq);
            } else {
                let mut legal_to_cap = Vec::with_capacity(cap_vars.len() + 1);
                legal_to_cap.push(-legal_ply_sq);
                legal_to_cap.extend(cap_vars);
                builder.add_clause(legal_to_cap);
            }
        }

        // (5) パス変数と合法手変数の関係を定義する
        // forall ply,sq: pass[ply] -> not legal[ply][sq]
        // forall ply: not pass[ply] -> (OR_sq legal[ply][sq])
        // forall ply,sq: play[ply][sq] -> legal[ply][sq]
        let mut not_pass_requires_legal = Vec::with_capacity(BOARD_SQUARES + 1);
        not_pass_requires_legal.push(pass_ply); // !pass[ply] のときに少なくとも1つの合法手を要求する
        for sq in 0..BOARD_SQUARES {
            let legal_ply_sq = vars.legal[ply][sq];
            not_pass_requires_legal.push(legal_ply_sq);
            builder.add_clause(vec![-pass_ply, -legal_ply_sq]); // pass[ply] が真なら legal[ply][sq] を偽にする
            builder.add_clause(vec![-legal_ply_sq, -pass_ply]); // legal[ply][sq] が真なら pass[ply] を偽にする
            builder.add_clause(vec![-vars.play[ply][sq], legal_ply_sq]); // play[ply][sq] が真なら legal[ply][sq] を真にする
        }
        builder.add_clause(not_pass_requires_legal);

        // (6) 反転原因変数と反転変数の意味を定義する
        // 各 i について、cause[ply][sq][i] は play[ply][p_i] と cap[ply][p_i][dir_i][k_i] の同時成立と同値である
        // ここで i は flip_sources[sq] の各要素を添字で表す
        // 1. cause[ply][sq][i] -> flip[ply][sq] を課し、flip[ply][sq] が真なら少なくとも1つの cause[ply][sq][i] が真であることを要求する
        // ただし flip_sources[sq] が空なら、この制約群は flip[ply][sq] を偽に固定する
        // 2. flip[ply][sq] が真なら layer の sq に相手色の石があることを要求する
        for sq in 0..BOARD_SQUARES {
            let flip_ply_sq = vars.flip[ply][sq];
            let cause_vars = &vars.cause[ply][sq];
            for (idx, cause_var) in cause_vars.iter().enumerate() {
                let source = flip_sources[sq][idx];
                let play_src = vars.play[ply][source.play_sq];
                let cap_src = vars.cap[ply][source.play_sq][source.dir][source.k];

                // cause[ply][sq][i] と (play_src and cap_src) の同値性
                builder.add_clause(vec![-*cause_var, play_src]);
                builder.add_clause(vec![-*cause_var, cap_src]);
                builder.add_clause(vec![-play_src, -cap_src, *cause_var]);

                // cause[ply][sq][i] が真なら flip[ply][sq] も真になる
                builder.add_clause(vec![-*cause_var, flip_ply_sq]);
            }
            if cause_vars.is_empty() {
                builder.add_unit(-flip_ply_sq);
            } else {
                let mut flip_implies_some_cause = Vec::with_capacity(cause_vars.len() + 1);
                flip_implies_some_cause.push(-flip_ply_sq);
                flip_implies_some_cause.extend_from_slice(cause_vars);
                builder.add_clause(flip_implies_some_cause);
            }

            // turn[layer]=black で flip[ply][sq] が真なら、layer の sq が white であることを要求する
            builder.add_implication_many(&[flip_ply_sq, turn_layer], vars.w[layer][sq]);
            // turn[layer]=white で flip[ply][sq] が真なら、layer の sq が black であることを要求する
            builder.add_implication_many(&[flip_ply_sq, -turn_layer], vars.b[layer][sq]);
        }

        // (7) layer から next_layer への盤面遷移を定義する
        // pass[ply] が真なら盤面をそのままコピーし、非 pass なら着手点と反転点を手番色へ更新する
        // 非 pass で着手点でも反転点でもないマスは、前層の値をそのまま維持する
        for sq in 0..BOARD_SQUARES {
            let play_ply_sq = vars.play[ply][sq];
            let flip_ply_sq = vars.flip[ply][sq];
            let b_cur = vars.b[layer][sq];
            let w_cur = vars.w[layer][sq];
            let b_next = vars.b[next_layer][sq];
            let w_next = vars.w[next_layer][sq];

            // pass[ply] が真のときに b と w を layer から next_layer へコピーする
            builder.add_implication_many(&[pass_ply, b_cur], b_next);
            builder.add_implication_many(&[pass_ply, -b_cur], -b_next);
            builder.add_implication_many(&[pass_ply, w_cur], w_next);
            builder.add_implication_many(&[pass_ply, -w_cur], -w_next);

            // turn[layer]=black かつ着手マスなら next_layer の sq を黒石に固定する
            builder.add_implication_many(&[turn_layer, -pass_ply, play_ply_sq], b_next);
            builder.add_implication_many(&[turn_layer, -pass_ply, play_ply_sq], -w_next);

            // turn[layer]=black かつ非着手の反転マスなら next_layer の sq を黒石に更新する
            builder
                .add_implication_many(&[turn_layer, -pass_ply, -play_ply_sq, flip_ply_sq], b_next);
            builder
                .add_implication_many(&[turn_layer, -pass_ply, -play_ply_sq, flip_ply_sq], -w_next);

            // turn[layer]=black かつ非着手・非反転マスで現在値を維持する
            builder.add_implication_many(
                &[turn_layer, -pass_ply, -play_ply_sq, -flip_ply_sq, b_cur],
                b_next,
            );
            builder.add_implication_many(
                &[turn_layer, -pass_ply, -play_ply_sq, -flip_ply_sq, -b_cur],
                -b_next,
            );
            builder.add_implication_many(
                &[turn_layer, -pass_ply, -play_ply_sq, -flip_ply_sq, w_cur],
                w_next,
            );
            builder.add_implication_many(
                &[turn_layer, -pass_ply, -play_ply_sq, -flip_ply_sq, -w_cur],
                -w_next,
            );

            // turn[layer]=white かつ着手マスなら next_layer の sq を白石に固定する
            builder.add_implication_many(&[-turn_layer, -pass_ply, play_ply_sq], w_next);
            builder.add_implication_many(&[-turn_layer, -pass_ply, play_ply_sq], -b_next);

            // turn[layer]=white かつ非着手の反転マスなら next_layer の sq を白石に更新する
            builder
                .add_implication_many(&[-turn_layer, -pass_ply, -play_ply_sq, flip_ply_sq], w_next);
            builder.add_implication_many(
                &[-turn_layer, -pass_ply, -play_ply_sq, flip_ply_sq],
                -b_next,
            );

            // turn[layer]=white かつ非着手・非反転マスで現在値を維持する
            builder.add_implication_many(
                &[-turn_layer, -pass_ply, -play_ply_sq, -flip_ply_sq, b_cur],
                b_next,
            );
            builder.add_implication_many(
                &[-turn_layer, -pass_ply, -play_ply_sq, -flip_ply_sq, -b_cur],
                -b_next,
            );
            builder.add_implication_many(
                &[-turn_layer, -pass_ply, -play_ply_sq, -flip_ply_sq, w_cur],
                w_next,
            );
            builder.add_implication_many(
                &[-turn_layer, -pass_ply, -play_ply_sq, -flip_ply_sq, -w_cur],
                -w_next,
            );
        }
    }

    builder.into_clauses()
}

fn solve_single_problem(
    vars: &SatVars,
    clauses: &[Vec<i32>],
    sat_timeout: Option<Duration>,
) -> Result<GoalSolveResult, String> {
    let mut solver = rustsat_kissat::Kissat::default();
    let mut cnf = Cnf::new();

    for raw_clause in clauses {
        let mut clause = Clause::new();
        for &lit in raw_clause {
            clause.add(to_rustsat_lit(lit));
        }
        cnf.add_clause(clause);
    }

    solver
        .add_cnf(cnf)
        .map_err(|err| format!("failed to add CNF to Kissat: {err}"))?;

    let timeout_guard = sat_timeout.map(|limit| {
        let interrupter = solver.interrupter();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            if done_rx.recv_timeout(limit).is_err() {
                interrupter.interrupt();
            }
        });
        (done_tx, worker)
    });

    let solve_result = solver.solve();

    if let Some((done_tx, worker)) = timeout_guard {
        let _ = done_tx.send(());
        let _ = worker.join();
    }

    match solve_result.map_err(|err| format!("kissat failed: {err}"))? {
        SolverResult::Sat => {
            let witness = decode_witness(vars, &solver)?;
            Ok(GoalSolveResult::Sat(witness))
        }
        SolverResult::Unsat => Ok(GoalSolveResult::Unsat),
        SolverResult::Interrupted => {
            if sat_timeout.is_some() {
                Ok(GoalSolveResult::Unknown)
            } else {
                Err("kissat solve was interrupted".to_string())
            }
        }
    }
}

fn decode_witness(
    vars: &SatVars,
    solver: &rustsat_kissat::Kissat<'_>,
) -> Result<SatWitness, String> {
    let mut boards = Vec::with_capacity(vars.h + 1);
    for layer in 0..=vars.h {
        let mut black = 0_u64;
        let mut white = 0_u64;
        for s in 0..BOARD_SQUARES {
            if value_of(solver, vars.b[layer][s])? {
                black |= 1_u64 << s;
            }
            if value_of(solver, vars.w[layer][s])? {
                white |= 1_u64 << s;
            }
        }
        boards.push(Board::new(black, white));
    }

    let mut plies = Vec::with_capacity(vars.h);
    for ply in 0..vars.h {
        let turn_black = value_of(solver, vars.turn[ply])?;
        let pass = value_of(solver, vars.pass[ply])?;
        if pass {
            plies.push(Ply {
                turn_black,
                action: PlyAction::Pass,
            });
            continue;
        }

        let mut played = None;
        for s in 0..BOARD_SQUARES {
            if value_of(solver, vars.play[ply][s])? {
                if played.is_some() {
                    return Err(format!(
                        "model decoding error: multiple play squares at ply={}",
                        ply
                    ));
                }
                played = Some(s);
            }
        }
        let sq =
            played.ok_or_else(|| format!("model decoding error: no play square at ply={}", ply))?;
        plies.push(Ply {
            turn_black,
            action: PlyAction::Play(sq),
        });
    }

    Ok(SatWitness {
        h: vars.h,
        plies,
        boards,
    })
}

fn print_sat_result(witness: &SatWitness, goal_index: usize, show_coords: bool, show_boards: bool) {
    println!("[{}] SAT (H={})", goal_index, witness.h);
    println!(
        "[{}] Record: {}",
        goal_index,
        build_record_string(&witness.plies)
    );

    if show_coords {
        for (ply_idx, ply) in witness.plies.iter().enumerate() {
            let side = if ply.turn_black { "Black" } else { "White" };
            match ply.action {
                PlyAction::Pass => {
                    println!("ply={} {} PASS", ply_idx, side);
                }
                PlyAction::Play(sq) => {
                    println!("ply={} {} {}", ply_idx, side, square_to_coord(sq));
                }
            }
        }
    }

    if show_boards {
        for (layer_idx, board) in witness.boards.iter().enumerate() {
            println!("layer[{}] {}", layer_idx, board.to_string());
        }
    }
}

fn build_record_string(plies: &[Ply]) -> String {
    let mut record = String::with_capacity(plies.len() * 3);
    for ply in plies {
        match ply.action {
            PlyAction::Pass => (),
            PlyAction::Play(sq) => record.push_str(&square_to_coord(sq)),
        }
    }
    record
}

// Reads a DIMACS variable value from the model, treating DontCare as false.
fn value_of(solver: &rustsat_kissat::Kissat<'_>, var: i32) -> Result<bool, String> {
    let lit = Lit::positive(var as u32);
    match solver
        .lit_val(lit)
        .map_err(|err| format!("failed to read model literal {var}: {err}"))?
    {
        TernaryVal::True => Ok(true),
        TernaryVal::False | TernaryVal::DontCare => Ok(false),
    }
}

// Converts a signed DIMACS literal into rustsat's Lit type.
fn to_rustsat_lit(raw_lit: i32) -> Lit {
    if raw_lit > 0 {
        Lit::positive(raw_lit as u32)
    } else {
        Lit::negative((-raw_lit) as u32)
    }
}

// Allocates `len` fresh variable ids as a 1D vector.
fn alloc_vector(next: &mut i32, len: usize) -> Vec<i32> {
    let mut out = vec![0_i32; len];
    for v in &mut out {
        *next += 1;
        *v = *next;
    }
    out
}

// Allocates `rows x cols` fresh variable ids as a 2D matrix.
fn alloc_matrix(next: &mut i32, rows: usize, cols: usize) -> Vec<Vec<i32>> {
    let mut out = vec![vec![0_i32; cols]; rows];
    for row in out.iter_mut().take(rows) {
        for cell in row.iter_mut().take(cols) {
            *next += 1;
            *cell = *next;
        }
    }
    out
}

// Precomputes ordered ray squares for every start square and direction.
fn precompute_rays() -> Rays {
    let mut rays = vec![vec![Vec::<usize>::new(); DIRS.len()]; BOARD_SQUARES];
    for (sq, sq_rays) in rays.iter_mut().enumerate().take(BOARD_SQUARES) {
        let x = (sq % 8) as i32;
        let y = (sq / 8) as i32;
        for (d, (dx, dy)) in DIRS.iter().enumerate() {
            let mut x1 = x + dx;
            let mut y1 = y + dy;
            while (0..8).contains(&x1) && (0..8).contains(&y1) {
                sq_rays[d].push((y1 * 8 + x1) as usize);
                x1 += dx;
                y1 += dy;
            }
        }
    }
    rays
}

// Precomputes all capture patterns that can flip each target square.
fn precompute_flip_sources(rays: &Rays) -> FlipSources {
    let mut sources: FlipSources = vec![Vec::new(); BOARD_SQUARES];
    for (play_sq, play_rays) in rays.iter().enumerate().take(BOARD_SQUARES) {
        for (d, ray) in play_rays.iter().enumerate().take(DIRS.len()) {
            let max_k = ray.len().saturating_sub(1);
            for k in 1..=max_k {
                for target in ray.iter().take(k) {
                    sources[*target].push(FlipSource { play_sq, dir: d, k });
                }
            }
        }
    }
    sources
}

#[inline(always)]
fn bit_is_set(bb: u64, sq: usize) -> bool {
    (bb >> sq) & 1 == 1
}

#[inline(always)]
fn square_to_coord(sq: usize) -> String {
    let file = (b'A' + (sq % 8) as u8) as char;
    let rank = sq / 8 + 1;
    format!("{file}{rank}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_coord_mapping() {
        assert_eq!(square_to_coord(0), "A1");
        assert_eq!(square_to_coord(7), "H1");
        assert_eq!(square_to_coord(8), "A2");
        assert_eq!(square_to_coord(63), "H8");
    }

    #[test]
    fn ray_lengths_from_corner() {
        let rays = precompute_rays();
        // A1
        assert_eq!(rays[0][0].len(), 7); // N
        assert_eq!(rays[0][2].len(), 7); // E
        assert_eq!(rays[0][1].len(), 7); // NE
        assert_eq!(rays[0][4].len(), 0); // S
        assert_eq!(rays[0][6].len(), 0); // W
        assert_eq!(rays[0][5].len(), 0); // SW
    }

    #[test]
    fn min_required_plies_matches_disc_difference() {
        let start = INITIAL_BOARD;
        let goal = Board::new(0x0000_0008_1808_0000, 0x0000_0010_0000_0000); // initial + D3

        assert_eq!(min_required_plies(start, goal), Some(1));
        assert_eq!(min_required_plies(start, start), Some(0));
        assert_eq!(min_required_plies(goal, start), None);
    }

    #[test]
    fn max_plies_allows_one_pass_per_move() {
        assert_eq!(max_plies_with_passes(0), 0);
        assert_eq!(max_plies_with_passes(1), 2);
        assert_eq!(max_plies_with_passes(60), 120);
    }

    #[test]
    fn record_string_concatenates_moves() {
        let plies = vec![
            Ply {
                turn_black: true,
                action: PlyAction::Play(19), // D3
            },
            Ply {
                turn_black: false,
                action: PlyAction::Play(34), // C5
            },
            Ply {
                turn_black: true,
                action: PlyAction::Pass,
            },
        ];
        assert_eq!(build_record_string(&plies), "D3C5");
    }
}
