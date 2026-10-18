//! Reachability between absolute positions, using relative boards at every SAT layer.
//! Exact queries solve one endpoint; symmetry queries enumerate representatives under
//! the stabilizer of the start. Every SAT model is replayed before an OK is returned.
use crate::io::{format_position, TriCategory, TriOutcome};
use crate::othello::{get_moves, validate_board, Board, BoardSymmetry, Position};
use crate::record::{format_actions, replay, GoalMatch, PlyAction, ReachabilityRecord};
use rayon::{prelude::*, ThreadPoolBuilder};
use rustsat::{
    instances::Cnf,
    solvers::{Interrupt, InterruptSolver, Solve, SolverResult},
    types::{Clause, Lit, TernaryVal},
};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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

    // output <-> AND(inputs), including signed input literals.
    fn add_conjunction(&mut self, output: i32, inputs: &[i32]) {
        for &input in inputs {
            self.add_implication(output, input);
        }
        self.add_implication_many(inputs, output);
    }

    fn into_clauses(self) -> Vec<Vec<i32>> {
        self.clauses
    }
}

/// layer - 盤面の状態添字, 範囲は 0..=max_h（総数は max_h+1）
/// ply - 1手ごとの状態遷移の添字, 範囲は 0..max_h（総数は max_h）
struct SatVars {
    max_h: usize,                 // ありうる所要手数 (ply数) の最大値
    var_count: i32,               // これまでに割り当てた DIMACS 変数番号の最大値
    player: Vec<Vec<i32>>,        // player[layer][sq]: 手番側の石
    opponent: Vec<Vec<i32>>,      // opponent[layer][sq]: 相手側の石
    active: Vec<i32>,             // active[ply]: ply < h なら真
    end: Vec<i32>,                // end[layer]: layer = h なら真
    pass: Vec<i32>,               // pass[ply]: ply がパス手か否か
    play: Vec<Vec<i32>>,          // play[ply][sq]: ply で sq に着手する
    legal: Vec<Vec<i32>>,         // legal[ply][sq]: sq が ply の合法手である
    flip: Vec<Vec<i32>>,          // flip[ply][sq]: sq が ply で反転する
    cap: Vec<Vec<Vec<Vec<i32>>>>, // cap[ply][sq][dir][k]: sq への着手が方向 dir で k 枚を挟む
    cause: Vec<Vec<Vec<i32>>>,    // cause[ply][sq][i]: ply 手目で sq が反転する原因 i が成立する
    reply_cap: Vec<Vec<Vec<Vec<i32>>>>,
    // reply_cap[ply][sq][dir][k]: 現手番が ply で pass するとき、相手が sq に着手し dir 方向に k 枚挟める
}

impl SatVars {
    fn new(max_h: usize, rays: &Rays, flip_sources: &FlipSources) -> Self {
        let mut next = 0_i32;

        let player = alloc_matrix(&mut next, max_h + 1, BOARD_SQUARES);
        let opponent = alloc_matrix(&mut next, max_h + 1, BOARD_SQUARES);
        let active = alloc_vector(&mut next, max_h);
        let end = alloc_vector(&mut next, max_h + 1);
        let pass = alloc_vector(&mut next, max_h);
        let play = alloc_matrix(&mut next, max_h, BOARD_SQUARES);
        let legal = alloc_matrix(&mut next, max_h, BOARD_SQUARES);
        let flip = alloc_matrix(&mut next, max_h, BOARD_SQUARES);
        let cap = alloc_cap_tensor(&mut next, max_h, rays);
        let reply_cap = alloc_cap_tensor(&mut next, max_h, rays);

        let mut cause: Vec<Vec<Vec<i32>>> = vec![vec![Vec::new(); BOARD_SQUARES]; max_h];
        for ply_causes in &mut cause {
            for (s, slot) in ply_causes.iter_mut().enumerate() {
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
            max_h,
            var_count: next,
            player,
            opponent,
            active,
            end,
            pass,
            play,
            legal,
            flip,
            cap,
            reply_cap,
            cause,
        }
    }
}

#[derive(Debug)]
struct SatWitness {
    actions: Vec<PlyAction>,
    boards: Vec<Board>,
}

/// Each goal is a relative board together with its absolute side to move.
/// Time limits apply separately to each candidate's solver call, not to a query.
/// CNF-only runs return no classifications and never call the outcome callback.
pub struct RunOptions {
    pub start: Position,
    pub goals: Vec<Position>,
    pub goal_match: GoalMatch,
    pub parallel_goals: usize,
    pub show_coords: bool,
    pub show_boards: bool,
    pub verbose: bool,
    pub cnf_dump_dir: Option<PathBuf>,
    pub cnf_dump_only: bool,
    pub sat_timeout_per_instance: Option<Duration>,
}

/// One classification per input occurrence, with a 1-based index.
/// Reachable always carries a replay-verified record of the requested query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GoalClassification {
    Reachable {
        record: ReachabilityRecord,
    },
    Unreachable {
        input_index: usize,
        goal: Position,
        goal_match: GoalMatch,
    },
    Unknown {
        input_index: usize,
        goal: Position,
        goal_match: GoalMatch,
    },
}

impl GoalClassification {
    pub fn goal(&self) -> Position {
        match self {
            Self::Reachable { record } => record.goal,
            Self::Unreachable { goal, .. } | Self::Unknown { goal, .. } => *goal,
        }
    }

    pub fn input_index(&self) -> usize {
        match self {
            Self::Reachable { record } => record.input_index,
            Self::Unreachable { input_index, .. } | Self::Unknown { input_index, .. } => {
                *input_index
            }
        }
    }

    pub fn goal_match(&self) -> GoalMatch {
        match self {
            Self::Reachable { record } => record.goal_match,
            Self::Unreachable { goal_match, .. } | Self::Unknown { goal_match, .. } => *goal_match,
        }
    }

    pub fn record(&self) -> Option<&ReachabilityRecord> {
        match self {
            Self::Reachable { record } => Some(record),
            _ => None,
        }
    }
}

impl TriOutcome for GoalClassification {
    fn category(&self) -> TriCategory {
        match self {
            Self::Reachable { .. } => TriCategory::Ok,
            Self::Unreachable { .. } => TriCategory::Ng,
            Self::Unknown { .. } => TriCategory::Unknown,
        }
    }
}

#[derive(Debug)]
enum GoalSolveResult {
    Sat(SatWitness),
    Unsat,
    Unknown,
}

/// Validate all positions before any output is created. Callbacks run serially
/// in completion order; the returned vector is in input order, including duplicates.
pub fn run_with_options<F>(
    opts: RunOptions,
    mut on_outcome: F,
) -> Result<Vec<GoalClassification>, String>
where
    F: FnMut(&GoalClassification) -> Result<(), String>,
{
    validate_board(&opts.start.board).map_err(|err| format!("invalid start: {err:?}"))?;
    if opts.goals.is_empty() {
        return Err("at least one goal is required".to_string());
    }
    for (index, goal) in opts.goals.iter().enumerate() {
        validate_board(&goal.board)
            .map_err(|err| format!("invalid goal[{}]: {err:?}", index + 1))?;
    }
    if opts.parallel_goals == 0 {
        return Err("parallel_goals must be positive".to_string());
    }
    if opts
        .sat_timeout_per_instance
        .is_some_and(|limit| limit.is_zero())
    {
        return Err("SAT timeout must be positive".to_string());
    }
    if opts.cnf_dump_only && opts.cnf_dump_dir.is_none() {
        return Err("CNF-only runs require an output directory".to_string());
    }
    if let Some(dir) = &opts.cnf_dump_dir {
        fs::create_dir_all(dir)
            .map_err(|e| format!("failed to create CNF directory {}: {e}", dir.display()))?;
    }

    let rays = precompute_rays();
    let flip_sources = precompute_flip_sources(&rays);
    let cancelled = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel();
    let pool = ThreadPoolBuilder::new()
        .num_threads(opts.parallel_goals.min(opts.goals.len()))
        .build()
        .map_err(|err| format!("failed to build rayon pool for layer_sat: {err}"))?;

    std::thread::scope(|scope| {
        let opts = &opts;
        let rays = &rays;
        let flip_sources = &flip_sources;
        let cancelled = &cancelled;
        scope.spawn(move || {
            pool.install(|| {
                opts.goals
                    .par_iter()
                    .enumerate()
                    .for_each_with(tx, |tx, (index, &goal)| {
                        if cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        let result = solve_query(
                            opts.start,
                            goal,
                            index + 1,
                            opts.goal_match,
                            |candidate, symmetry| {
                                let path = opts.cnf_dump_dir.as_ref().map(|dir| {
                                    dir.join(format!("goal_{:04}_sym_{}.cnf", index + 1, symmetry))
                                });
                                check_exact_reachability(
                                    opts.start,
                                    candidate,
                                    path.as_deref(),
                                    opts.cnf_dump_only,
                                    opts.sat_timeout_per_instance,
                                    opts.verbose,
                                    rays,
                                    flip_sources,
                                )
                            },
                        )
                        .map(|outcome| (index, outcome))
                        .map_err(|err| {
                            cancelled.store(true, Ordering::Relaxed);
                            format!("goal[{}] {}: {err}", index + 1, format_position(goal))
                        });
                        if tx.send(result).is_err() {
                            cancelled.store(true, Ordering::Relaxed);
                        }
                    });
            });
        });

        let mut ordered_outcomes = vec![None; opts.goals.len()];
        let mut first_error = None;
        for message in rx {
            match message {
                Err(err) => {
                    if first_error.is_none() {
                        cancelled.store(true, Ordering::Relaxed);
                        first_error = Some(err);
                    }
                }
                Ok((index, Some(outcome))) if first_error.is_none() => {
                    if let Err(err) = print_result(&outcome, opts.show_coords, opts.show_boards)
                        .and_then(|()| on_outcome(&outcome))
                    {
                        cancelled.store(true, Ordering::Relaxed);
                        first_error = Some(err);
                    }
                    ordered_outcomes[index] = Some(outcome);
                }
                Ok(_) => {}
            }
        }
        if let Some(err) = first_error {
            return Err(err);
        }
        if opts.cnf_dump_only {
            return Ok(Vec::new());
        }
        ordered_outcomes
            .into_iter()
            .enumerate()
            .map(|(index, outcome)| {
                outcome.ok_or_else(|| format!("missing result for input {}", index + 1))
            })
            .collect()
    })
}

// None is reserved for CNF-only generation. Execution errors propagate even
// when another candidate has already timed out.
fn solve_query(
    start: Position,
    goal: Position,
    input_index: usize,
    goal_match: GoalMatch,
    mut solve: impl FnMut(Position, BoardSymmetry) -> Result<Option<GoalSolveResult>, String>,
) -> Result<Option<GoalClassification>, String> {
    let candidates = match goal_match {
        GoalMatch::Exact => vec![(BoardSymmetry::Identity, goal)],
        GoalMatch::SymmetryOrbit => all_goals_to_solve(start, goal),
    };
    let mut unknown = false;
    let mut classified = false;
    for (symmetry, candidate) in candidates {
        let Some(result) = solve(candidate, symmetry)? else {
            continue;
        };
        classified = true;
        match result {
            GoalSolveResult::Sat(witness) => {
                verify_witness(&witness, start, candidate)?;
                let record = ReachabilityRecord {
                    input_index,
                    start,
                    goal,
                    goal_match,
                    symmetry,
                    actions: witness.actions,
                };
                record.verify()?;
                return Ok(Some(GoalClassification::Reachable { record }));
            }
            GoalSolveResult::Unsat => {}
            GoalSolveResult::Unknown => unknown = true,
        }
    }
    if !classified {
        Ok(None)
    } else if unknown {
        Ok(Some(GoalClassification::Unknown {
            input_index,
            goal,
            goal_match,
        }))
    } else {
        Ok(Some(GoalClassification::Unreachable {
            input_index,
            goal,
            goal_match,
        }))
    }
}

// This core only knows the exact start and endpoint. No symmetry or match
// condition participates in encoding or model verification.
#[allow(clippy::too_many_arguments)]
fn check_exact_reachability(
    start: Position,
    goal: Position,
    cnf_path: Option<&Path>,
    cnf_dump_only: bool,
    timeout: Option<Duration>,
    verbose: bool,
    rays: &Rays,
    flip_sources: &FlipSources,
) -> Result<Option<GoalSolveResult>, String> {
    let Some(min_plies) = min_required_plies(start.board, goal.board) else {
        if let Some(path) = cnf_path {
            dump_dimacs_cnf(path, 0, &[vec![]])?;
        }
        return Ok((!cnf_dump_only).then_some(GoalSolveResult::Unsat));
    };
    let max_plies = max_plies_with_passes(start.board, min_plies);
    if !(min_plies..=max_plies).any(|h| endpoint_turn_matches(start, goal, h)) {
        if let Some(path) = cnf_path {
            dump_dimacs_cnf(path, 0, &[vec![]])?;
        }
        return Ok((!cnf_dump_only).then_some(GoalSolveResult::Unsat));
    }
    let build_started = Instant::now();
    let vars = SatVars::new(max_plies, rays, flip_sources);
    let clauses = encode_problem(&vars, start, goal, min_plies, rays, flip_sources);
    let build_seconds = build_started.elapsed().as_secs_f64();
    if let Some(path) = cnf_path {
        dump_dimacs_cnf(path, vars.var_count as usize, &clauses)?;
    }
    if verbose {
        println!(
            "[encoding] goal={} range={}..={} vars={} clauses={} build_secs={:.6}",
            format_position(goal),
            min_plies,
            max_plies,
            vars.var_count,
            clauses.len(),
            build_seconds
        );
    }
    if cnf_dump_only {
        return Ok(None);
    }
    let solve_started = Instant::now();
    let result = solve_single_problem(&vars, &clauses, timeout)?;
    if verbose {
        println!(
            "[solver] goal={} solve_secs={:.6}",
            format_position(goal),
            solve_started.elapsed().as_secs_f64()
        );
    }
    if let GoalSolveResult::Sat(witness) = &result {
        verify_witness(witness, start, goal)?;
    }
    Ok(Some(result))
}

fn dump_dimacs_cnf(path: &Path, var_count: usize, clauses: &[Vec<i32>]) -> Result<(), String> {
    let file =
        File::create(path).map_err(|e| format!("failed to create CNF {}: {e}", path.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "p cnf {} {}", var_count, clauses.len())
        .map_err(|e| format!("failed to write CNF header: {e}"))?;
    for clause in clauses {
        for lit in clause {
            write!(writer, "{lit} ").map_err(|e| format!("failed to write CNF clause: {e}"))?;
        }
        writeln!(writer, "0").map_err(|e| format!("failed to write CNF clause: {e}"))?;
    }
    writer
        .flush()
        .map_err(|e| format!("failed to flush CNF {}: {e}", path.display()))
}

fn min_required_plies(start: Board, goal: Board) -> Option<usize> {
    goal.popcount()
        .checked_sub(start.popcount())
        .map(|m| m as usize)
}

fn max_plies_with_passes(start: Board, placements: usize) -> usize {
    // Passes cannot be consecutive. Charge every pass except a possible first
    // one to its preceding placement. Thus passes <= placements + delta.
    let delta = usize::from(get_moves(start.player, start.opponent) == 0);
    2 * placements + delta
}

fn endpoint_turn_matches(start: Position, goal: Position, layer: usize) -> bool {
    layer.is_multiple_of(2) == (start.side_to_move == goal.side_to_move)
}

fn all_goals_to_solve(start: Position, goal: Position) -> Vec<(BoardSymmetry, Position)> {
    let stabilizer: Vec<_> = BoardSymmetry::ALL
        .into_iter()
        .filter(|symmetry| symmetry.apply_position(start) == start)
        .collect();
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for symmetry in BoardSymmetry::ALL {
        let candidate = symmetry.apply_position(goal);
        // Identity is in the stabilizer, so the minimum always exists.
        let canonical = stabilizer
            .iter()
            .map(|sym| sym.apply(candidate.board))
            .min()
            .expect("identity fixes every start");
        if seen.insert(canonical) {
            candidates.push((symmetry, candidate));
        }
    }
    candidates
}

fn encode_problem(
    vars: &SatVars,
    start: Position,
    goal: Position,
    min_plies: usize,
    rays: &Rays,
    flip_sources: &FlipSources,
) -> Vec<Vec<i32>> {
    let mut builder = ClauseBuilder::default();
    add_range_selector_constraints(&mut builder, &vars.active, &vars.end, min_plies);

    // (1) Initial relative board and disjoint occupancy at every layer.
    for sq in 0..BOARD_SQUARES {
        builder.add_unit(if bit_is_set(start.board.player, sq) {
            vars.player[0][sq]
        } else {
            -vars.player[0][sq]
        });
        builder.add_unit(if bit_is_set(start.board.opponent, sq) {
            vars.opponent[0][sq]
        } else {
            -vars.opponent[0][sq]
        });
    }
    for layer in 0..=vars.max_h {
        for sq in 0..BOARD_SQUARES {
            builder.add_clause(vec![-vars.player[layer][sq], -vars.opponent[layer][sq]]);
        }
        // (2) Only endpoints with the required absolute turn may be selected.
        if !endpoint_turn_matches(start, goal, layer) {
            builder.add_unit(-vars.end[layer]);
            continue;
        }
        for sq in 0..BOARD_SQUARES {
            builder.add_implication(
                vars.end[layer],
                if bit_is_set(goal.board.player, sq) {
                    vars.player[layer][sq]
                } else {
                    -vars.player[layer][sq]
                },
            );
            builder.add_implication(
                vars.end[layer],
                if bit_is_set(goal.board.opponent, sq) {
                    vars.opponent[layer][sq]
                } else {
                    -vars.opponent[layer][sq]
                },
            );
        }
    }

    for ply in 0..vars.max_h {
        let active = vars.active[ply];
        let pass = vars.pass[ply];

        // (3) Exactly one action when active; no action after the endpoint.
        let mut action_required = vec![-active, pass];
        action_required.extend_from_slice(&vars.play[ply]);
        builder.add_clause(action_required);
        builder.add_implication(pass, active);
        for (sq, &play) in vars.play[ply].iter().enumerate() {
            builder.add_implication(play, active);
            builder.add_clause(vec![-pass, -play]);
            for &earlier in &vars.play[ply][..sq] {
                builder.add_clause(vec![-play, -earlier]);
            }
        }

        let mut pass_requires_reply = vec![-pass];
        for (sq, square_rays) in rays.iter().enumerate() {
            let player = vars.player[ply][sq];
            let opponent = vars.opponent[ply][sq];
            let legal = vars.legal[ply][sq];
            let mut legal_requires_capture = vec![-legal];

            // (4) cap <-> active AND empty AND opponent run AND player anchor.
            // legal <-> OR(cap). These variables are false on inactive layers.
            for (dir, ray) in square_rays.iter().enumerate() {
                for k in 1..ray.len() {
                    let cap = vars.cap[ply][sq][dir][k];
                    let mut capture = vec![active, -player, -opponent];
                    capture.extend(ray[..k].iter().map(|&r| vars.opponent[ply][r]));
                    capture.push(vars.player[ply][ray[k]]);
                    builder.add_conjunction(cap, &capture);
                    builder.add_implication(cap, legal);
                    legal_requires_capture.push(cap);

                    // (5) The opponent must be able to move for a pass to be
                    // legal. Its capture conditions exchange player/opponent.
                    let reply_cap = vars.reply_cap[ply][sq][dir][k];
                    let mut reply = vec![active, -player, -opponent];
                    reply.extend(ray[..k].iter().map(|&r| vars.player[ply][r]));
                    reply.push(vars.opponent[ply][ray[k]]);
                    builder.add_conjunction(reply_cap, &reply);
                    pass_requires_reply.push(reply_cap);
                }
            }
            builder.add_clause(legal_requires_capture);
            builder.add_implication(pass, -legal);
            builder.add_implication(vars.play[ply][sq], legal);
        }
        builder.add_clause(pass_requires_reply);

        // (6) A square flips iff the chosen placement captures it in some ray.
        for (sq, sources) in flip_sources.iter().enumerate() {
            let flip = vars.flip[ply][sq];
            let mut flip_requires_cause = vec![-flip];
            for (source, &cause) in sources.iter().zip(&vars.cause[ply][sq]) {
                builder.add_conjunction(
                    cause,
                    &[
                        vars.play[ply][source.play_sq],
                        vars.cap[ply][source.play_sq][source.dir][source.k],
                    ],
                );
                builder.add_implication(cause, flip);
                flip_requires_cause.push(cause);
            }
            builder.add_clause(flip_requires_cause);
            builder.add_implication(flip, vars.opponent[ply][sq]);

            // (7) P' = O \ F; O' = P union F union M.
            // A pass and an inactive transition both have M=F=empty, hence
            // exchange relative viewpoints. Only active actions form a record.
            let next_player = vars.player[ply + 1][sq];
            let next_opponent = vars.opponent[ply + 1][sq];
            builder.add_conjunction(next_player, &[vars.opponent[ply][sq], -flip]);
            let additions = [vars.player[ply][sq], flip, vars.play[ply][sq]];
            for added in additions {
                builder.add_implication(added, next_opponent);
            }
            builder.add_clause(vec![
                -next_opponent,
                additions[0],
                additions[1],
                additions[2],
            ]);
        }
    }
    builder.into_clauses()
}

// h の範囲に関する制約を課す
//
// 1. ∀ ply < min_plies: active[ply]
// 2. ∀ ply < max_h - 1: active[ply + 1] -> active[ply]
// 3. end[0] <-> ¬active[0]
// 4. ∀ 1 <= layer < max_h:
//      end[layer] <-> (active[layer - 1] ∧ ¬active[layer])
// 5. end[max_h] <-> active[max_h - 1]
//
// 1により min_plies <= h が保証される。
// 2により active = (true, ..., true, false, ..., false) という形に限られる。
// 3-5 により、ある layer で end[layer] が真であることと h = layer は同値になる。
//
// ex1) h = 2, max_h = 5 のとき
//   active = [T, T, F, F, F]
//   end    = [F, F, T, F, F, F]
// ex2) h = max_h = 5 のとき
//   active = [T, T, T, T, T]
//   end    = [F, F, F, F, F, T]
fn add_range_selector_constraints(
    builder: &mut ClauseBuilder,
    active: &[i32],
    end: &[i32],
    min_plies: usize,
) {
    // max_h=0 -> end[0]
    if active.is_empty() {
        builder.add_unit(end[0]);
        return;
    }

    for (ply, &active_ply) in active.iter().enumerate() {
        if ply < min_plies {
            builder.add_unit(active_ply);
        }
        if ply + 1 < active.len() {
            builder.add_implication(active[ply + 1], active_ply);
        }
    }

    builder.add_clause(vec![-end[0], -active[0]]);
    builder.add_clause(vec![active[0], end[0]]);
    for layer in 1..active.len() {
        builder.add_implication(end[layer], active[layer - 1]);
        builder.add_clause(vec![-end[layer], -active[layer]]);
        builder.add_clause(vec![-active[layer - 1], active[layer], end[layer]]);
    }
    builder.add_implication(end[active.len()], active[active.len() - 1]);
    builder.add_implication(active[active.len() - 1], end[active.len()]);
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
        worker
            .join()
            .map_err(|_| "SAT timeout worker panicked".to_string())?;
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

fn verify_witness(witness: &SatWitness, start: Position, goal: Position) -> Result<(), String> {
    let positions = replay(start, &witness.actions)?;
    if positions.len() != witness.boards.len() {
        return Err("model verification failed: number of layers differs from replay".to_string());
    }
    for (layer, (position, board)) in positions.iter().zip(&witness.boards).enumerate() {
        if position.board != *board {
            return Err(format!(
                "model verification failed: layer {layer} differs from replay"
            ));
        }
    }
    if positions.last() != Some(&goal) {
        return Err(
            "model verification failed: endpoint position differs from requested goal".to_string(),
        );
    }
    Ok(())
}

fn decode_witness(
    vars: &SatVars,
    solver: &rustsat_kissat::Kissat<'_>,
) -> Result<SatWitness, String> {
    let h = decode_selected_h(vars, solver)?;
    let mut boards = Vec::with_capacity(h + 1);
    for layer in 0..=h {
        let mut player = 0_u64;
        let mut opponent = 0_u64;
        for sq in 0..BOARD_SQUARES {
            if value_of(solver, vars.player[layer][sq])? {
                player |= 1_u64 << sq;
            }
            if value_of(solver, vars.opponent[layer][sq])? {
                opponent |= 1_u64 << sq;
            }
        }
        boards.push(Board::new(player, opponent));
    }
    let mut actions = Vec::with_capacity(h);
    for ply in 0..h {
        let mut action = if value_of(solver, vars.pass[ply])? {
            Some(PlyAction::Pass)
        } else {
            None
        };
        for sq in 0..BOARD_SQUARES {
            if value_of(solver, vars.play[ply][sq])? {
                if action.is_some() {
                    return Err(format!(
                        "model decoding error: multiple actions at ply {ply}"
                    ));
                }
                action = Some(PlyAction::Play(sq));
            }
        }
        actions
            .push(action.ok_or_else(|| format!("model decoding error: no action at ply {ply}"))?);
    }
    Ok(SatWitness { actions, boards })
}

// Read the unique selected endpoint. Never infer missing model values.
fn decode_selected_h(vars: &SatVars, solver: &rustsat_kissat::Kissat<'_>) -> Result<usize, String> {
    let mut selected = None;
    for layer in 0..=vars.max_h {
        if value_of(solver, vars.end[layer])? {
            if selected.is_some() {
                return Err("model decoding error: multiple end layers selected".to_string());
            }
            selected = Some(layer);
        }
    }
    selected.ok_or_else(|| "model decoding error: no end layer selected".to_string())
}

fn print_result(
    outcome: &GoalClassification,
    show_coords: bool,
    show_boards: bool,
) -> Result<(), String> {
    match outcome {
        GoalClassification::Reachable { record } => {
            println!(
                "[{}] SAT (H={}, symmetry={})",
                record.input_index,
                record.actions.len(),
                record.symmetry
            );
            if show_coords {
                println!(
                    "[{}] Record: {}",
                    record.input_index,
                    format_actions(&record.actions)
                );
            }
            if show_boards {
                for (layer, position) in record.verify()?.into_iter().enumerate() {
                    println!("layer[{layer}] {}", format_position(position));
                }
            }
        }
        GoalClassification::Unreachable { input_index, .. } => println!("[{input_index}] UNSAT"),
        GoalClassification::Unknown { input_index, .. } => println!("[{input_index}] UNKNOWN"),
    }
    Ok(())
}

fn value_of(solver: &rustsat_kissat::Kissat<'_>, var: i32) -> Result<bool, String> {
    let lit = Lit::positive(var as u32);
    match solver
        .lit_val(lit)
        .map_err(|err| format!("failed to read model literal {var}: {err}"))?
    {
        TernaryVal::True => Ok(true),
        TernaryVal::False => Ok(false),
        TernaryVal::DontCare => Err(format!("model decoding error: literal {var} has no value")),
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

fn alloc_cap_tensor(next: &mut i32, h: usize, rays: &Rays) -> Vec<Vec<Vec<Vec<i32>>>> {
    let mut cap = vec![vec![vec![Vec::new(); DIRS.len()]; BOARD_SQUARES]; h];
    for ply_caps in &mut cap {
        for (square_caps, square_rays) in ply_caps.iter_mut().zip(rays) {
            for (direction_caps, ray) in square_caps.iter_mut().zip(square_rays) {
                let max_k = ray.len().saturating_sub(1);
                // k = 0 is a dummy slot. Real capture variables are k >= 1 only.
                let mut ks = vec![0_i32; max_k + 1];
                for slot in ks.iter_mut().take(max_k + 1).skip(1) {
                    *next += 1;
                    *slot = *next;
                }
                *direction_caps = ks;
            }
        }
    }
    cap
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

#[cfg(test)]
mod tests {
    use super::*;

    // The orbit of the reflected initial position has two representatives
    // under the stabilizer of the fixed standard initial position.
    fn aggregate(
        mut results: Vec<Result<GoalSolveResult, String>>,
    ) -> Result<Option<GoalClassification>, String> {
        results.reverse();
        solve_query(
            Position::initial(),
            BoardSymmetry::MirrorHorizontal.apply_position(Position::initial()),
            1,
            GoalMatch::SymmetryOrbit,
            |_, _| Ok(Some(results.pop().expect("candidate count")?)),
        )
    }

    #[test]
    fn candidate_aggregation_preserves_unknown_and_errors() {
        let all_ng = aggregate(vec![Ok(GoalSolveResult::Unsat), Ok(GoalSolveResult::Unsat)])
            .unwrap()
            .unwrap();
        assert_eq!(all_ng.category(), TriCategory::Ng);
        for results in [
            vec![Ok(GoalSolveResult::Unknown), Ok(GoalSolveResult::Unsat)],
            vec![Ok(GoalSolveResult::Unsat), Ok(GoalSolveResult::Unknown)],
        ] {
            assert_eq!(
                aggregate(results).unwrap().unwrap().category(),
                TriCategory::Unknown
            );
        }
        assert!(aggregate(vec![
            Ok(GoalSolveResult::Unknown),
            Err("solver failure".to_string())
        ])
        .is_err());
    }

    #[test]
    fn candidate_sat_after_unknown_is_verified_and_wins() {
        let outcome = aggregate(vec![
            Ok(GoalSolveResult::Unknown),
            Ok(GoalSolveResult::Sat(SatWitness {
                actions: vec![],
                boards: vec![Position::initial().board],
            })),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(outcome.category(), TriCategory::Ok);
        let record = outcome.record().unwrap();
        assert_ne!(record.symmetry, BoardSymmetry::Identity);
        assert_eq!(record.verify().unwrap(), vec![Position::initial()]);
    }

    #[test]
    fn corrupt_model_layers_and_endpoint_are_execution_errors() {
        let start = Position::initial();
        assert!(verify_witness(
            &SatWitness {
                actions: vec![],
                boards: vec![start.board.swapped()]
            },
            start,
            start
        )
        .is_err());
        let wrong_turn = Position {
            side_to_move: start.side_to_move.opposite(),
            ..start
        };
        assert!(verify_witness(
            &SatWitness {
                actions: vec![],
                boards: vec![start.board]
            },
            start,
            wrong_turn
        )
        .is_err());
        assert!(aggregate(vec![
            Ok(GoalSolveResult::Unknown),
            Ok(GoalSolveResult::Sat(SatWitness {
                actions: vec![PlyAction::Pass],
                boards: vec![start.board, start.board.swapped()],
            }))
        ])
        .is_err());
    }

    #[test]
    fn solver_timeout_is_unknown() {
        // Pigeonhole principle: 12 pigeons cannot occupy 11 holes with at most
        // one pigeon per hole. It is unsatisfiable independently of Othello.
        let mut clauses = Vec::new();
        let variable = |pigeon: i32, hole: i32| 1 + pigeon * 11 + hole;
        for pigeon in 0..12 {
            clauses.push((0..11).map(|hole| variable(pigeon, hole)).collect());
        }
        for hole in 0..11 {
            for pigeon in 0..12 {
                for earlier in 0..pigeon {
                    clauses.push(vec![-variable(pigeon, hole), -variable(earlier, hole)]);
                }
            }
        }
        let rays = precompute_rays();
        let vars = SatVars::new(0, &rays, &precompute_flip_sources(&rays));
        assert!(matches!(
            solve_single_problem(&vars, &clauses, Some(Duration::from_millis(1))).unwrap(),
            GoalSolveResult::Unknown
        ));
    }
}
