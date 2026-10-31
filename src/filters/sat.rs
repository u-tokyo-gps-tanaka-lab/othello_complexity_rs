//! Reachability decisions and witness searches through selectable SAT encodings.

mod cnf;
pub mod flip;
mod nopass;
mod placement;
mod ply;
mod rays;

use crate::io::format_position;
use crate::othello::{get_moves, validate_board, Position};
use crate::query::{candidates, solve_query, ExactOutcome, GoalClassification};
use crate::record::{format_actions, replay, GoalMatch, PlyAction, PositionTransform};
use crate::search::bfs::{retrospective_bfs, BfsOutcome};
use crate::search::core::SearchOutcome;
use crate::search::forward::ForwardTable;
use rayon::{prelude::*, ThreadPoolBuilder};
use rustsat::{
    instances::Cnf,
    solvers::{Interrupt, InterruptSolver, Solve, SolverResult},
    types::Clause,
};
use rustsat_kissat::Interrupter;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SatEncoding {
    Ply,
    Placement,
    PlacementColumns,
    PlacementRays,
    NoPassWitness,
}

impl SatEncoding {
    pub const ALL: [Self; 5] = [
        Self::Ply,
        Self::Placement,
        Self::PlacementColumns,
        Self::PlacementRays,
        Self::NoPassWitness,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ply => "ply",
            Self::Placement => "placement",
            Self::PlacementColumns => "placement-columns",
            Self::PlacementRays => "placement-rays",
            Self::NoPassWitness => "nopass-witness",
        }
    }

    /// Whether absence of a satisfying assignment proves full unreachability.
    pub const fn proves_unreachable(self) -> bool {
        !matches!(self, Self::NoPassWitness)
    }
}

impl fmt::Display for SatEncoding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for SatEncoding {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|encoding| encoding.as_str() == value)
            .ok_or_else(|| format!("unknown SAT encoding: {value}"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SatPortfolio {
    encodings: Vec<SatEncoding>,
}

impl SatPortfolio {
    pub fn new(encodings: Vec<SatEncoding>) -> Result<Self, String> {
        if encodings.is_empty() {
            return Err("SAT portfolio must contain at least one encoding".to_owned());
        }
        for (index, encoding) in encodings.iter().enumerate() {
            if encodings[..index].contains(encoding) {
                return Err(format!("duplicate SAT encoding in portfolio: {encoding}"));
            }
        }
        Ok(Self { encodings })
    }

    pub fn encodings(&self) -> &[SatEncoding] {
        &self.encodings
    }

    pub fn proves_unreachable(&self) -> bool {
        self.encodings
            .iter()
            .any(|encoding| encoding.proves_unreachable())
    }
}

impl fmt::Display for SatPortfolio {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, encoding) in self.encodings.iter().enumerate() {
            if index != 0 {
                formatter.write_str("+")?;
            }
            encoding.fmt(formatter)?;
        }
        Ok(())
    }
}

impl Serialize for SatPortfolio {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SatPortfolio {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl FromStr for SatPortfolio {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err("SAT portfolio must contain at least one encoding".to_owned());
        }
        value
            .split('+')
            .map(SatEncoding::from_str)
            .collect::<Result<Vec<_>, _>>()
            .and_then(Self::new)
    }
}

impl From<SatEncoding> for SatPortfolio {
    fn from(encoding: SatEncoding) -> Self {
        Self {
            encodings: vec![encoding],
        }
    }
}

/// A bounded exact reverse search that runs before every SAT call.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReverseBfs {
    /// Stone count of the shared forward table, between the start count and 64.
    pub discs: u32,
    /// Maximum registered reverse positions per candidate in the breadth-first search.
    pub state_budget: usize,
}

/// Each goal is a relative board together with its absolute side to move.
/// Time limits apply separately to each candidate's solver call, not to a query.
/// CNF-only runs return no classifications and never call the outcome callback.
pub struct RunOptions {
    pub start: Position,
    pub goals: Vec<Position>,
    pub goal_match: GoalMatch,
    pub encodings: SatPortfolio,
    pub seeds: usize,
    pub parallel_goals: usize,
    pub show_coords: bool,
    pub show_boards: bool,
    pub verbose: bool,
    pub cnf_dump_dir: Option<PathBuf>,
    pub cnf_dump_only: bool,
    pub sat_timeout_per_instance: Option<Duration>,
    pub reverse_bfs: Option<ReverseBfs>,
}

pub(crate) trait ModelDecoder: Send + Sync {
    fn decode_and_verify(
        &self,
        solver: &rustsat_kissat::Kissat<'_>,
    ) -> Result<Vec<PlyAction>, String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProofScope {
    Complete,
    WitnessOnly,
}

pub(crate) struct CnfProblem {
    variable_count: usize,
    clauses: Vec<Vec<i32>>,
    decoder: Box<dyn ModelDecoder + Send + Sync>,
    proof_scope: ProofScope,
}

impl CnfProblem {
    pub(crate) fn new(
        variable_count: usize,
        clauses: Vec<Vec<i32>>,
        decoder: impl ModelDecoder + 'static,
        proof_scope: ProofScope,
    ) -> Self {
        Self {
            variable_count,
            clauses,
            decoder: Box::new(decoder),
            proof_scope,
        }
    }

    pub(crate) fn variable_count(&self) -> usize {
        self.variable_count
    }

    pub(crate) fn clause_count(&self) -> usize {
        self.clauses.len()
    }
}

pub(crate) enum EncodingResult {
    Trivial(ExactOutcome),
    Cnf(CnfProblem),
}

/// Validate settings that do not depend on the goal list.
pub fn validate_options(opts: &RunOptions) -> Result<(), String> {
    validate_board(&opts.start.board).map_err(|err| format!("invalid start: {err:?}"))?;
    if opts.parallel_goals == 0 {
        return Err("parallel_goals must be positive".to_owned());
    }
    if opts.seeds == 0 {
        return Err("seeds must be positive".to_owned());
    }
    if opts
        .sat_timeout_per_instance
        .is_some_and(|limit| limit.is_zero())
    {
        return Err("SAT timeout must be positive".to_owned());
    }
    if opts.cnf_dump_only && opts.cnf_dump_dir.is_none() {
        return Err("CNF-only runs require an output directory".to_owned());
    }
    if let Some(bfs) = opts.reverse_bfs {
        if !opts.encodings.proves_unreachable() {
            return Err(
                "reverse BFS requires a complete encoding in the portfolio; \
                 nopass-witness alone cannot record its results"
                    .to_owned(),
            );
        }
        if bfs.state_budget == 0 {
            return Err("reverse BFS state budget must be positive".to_owned());
        }
        if !(opts.start.board.popcount()..=64).contains(&bfs.discs) {
            return Err(
                "reverse BFS discs must be between the start stone count and 64".to_owned(),
            );
        }
    }
    Ok(())
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
    validate_options(&opts)?;
    for (index, goal) in opts.goals.iter().enumerate() {
        validate_board(&goal.board)
            .map_err(|err| format!("invalid goal[{}]: {err:?}", index + 1))?;
    }
    if let Some(dir) = &opts.cnf_dump_dir {
        fs::create_dir_all(dir).map_err(|error| {
            format!("failed to create CNF directory {}: {error}", dir.display())
        })?;
    }

    if opts.goals.is_empty() {
        return Ok(Vec::new());
    }
    // One immutable forward table serves every parallel goal. Its boundary is
    // capped by the largest goal but never placed below the start position.
    let forward_table = match opts.reverse_bfs {
        Some(bfs) if !opts.cnf_dump_only => {
            let largest = opts
                .goals
                .iter()
                .map(|goal| goal.board.popcount())
                .max()
                .expect("goals are nonempty");
            Some(
                ForwardTable::from_start(
                    opts.start,
                    bfs.discs.min(largest).max(opts.start.board.popcount()),
                    None,
                )
                .map_err(|error| format!("failed to build the forward table: {error}"))?,
            )
        }
        _ => None,
    };
    let cancelled = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel();
    let pool = ThreadPoolBuilder::new()
        .num_threads(opts.parallel_goals.min(opts.goals.len()))
        .build()
        .map_err(|error| format!("failed to build rayon pool for SAT: {error}"))?;

    std::thread::scope(|scope| {
        let opts = &opts;
        let cancelled = &cancelled;
        let forward_table = forward_table.as_ref();
        scope.spawn(move || {
            pool.install(|| {
                opts.goals
                    .par_iter()
                    .enumerate()
                    .for_each_with(tx, |tx, (index, &goal)| {
                        if cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        let input_started = Instant::now();
                        let result = solve_goal(opts, forward_table, index, goal);
                        if let Ok(Some(outcome)) = &result {
                            print_classification_verbose(
                                opts.verbose,
                                index + 1,
                                outcome,
                                input_started.elapsed().as_secs_f64(),
                            );
                        }
                        let result = result.map(|outcome| (index, outcome)).map_err(|error| {
                            cancelled.store(true, Ordering::Relaxed);
                            format!("goal[{}] {}: {error}", index + 1, format_position(goal))
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
                Err(error) => {
                    if first_error.is_none() {
                        cancelled.store(true, Ordering::Relaxed);
                        first_error = Some(error);
                    }
                }
                Ok((index, Some(outcome))) if first_error.is_none() => {
                    if let Err(error) = print_result(&outcome, opts.show_coords, opts.show_boards)
                        .and_then(|()| on_outcome(&outcome))
                    {
                        cancelled.store(true, Ordering::Relaxed);
                        first_error = Some(error);
                    }
                    ordered_outcomes[index] = Some(outcome);
                }
                Ok(_) => {}
            }
        }
        if let Some(error) = first_error {
            return Err(error);
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

/// Classify one input goal. The reverse BFS runs for every nontrivial
/// candidate before any SAT encoding is built, so a witness for one candidate
/// can spare the other candidates their solver calls.
fn solve_goal(
    opts: &RunOptions,
    forward_table: Option<&ForwardTable>,
    index: usize,
    goal: Position,
) -> Result<Option<GoalClassification>, String> {
    let mut bfs_results: HashMap<Position, Option<ExactOutcome>> = HashMap::new();
    let mut bfs_seconds = HashMap::new();
    let mut witnessed = false;
    if let Some((bfs, table)) = opts.reverse_bfs.zip(forward_table) {
        for (_, candidate) in candidates(opts.start, goal, opts.goal_match) {
            if bfs_results.contains_key(&candidate)
                || zero_placement_outcome(opts.start, candidate)?.is_some()
            {
                continue;
            }
            let (outcome, seconds) = run_bfs(candidate, table, bfs, index + 1, opts.verbose)?;
            let found = matches!(outcome, Some(ExactOutcome::Reachable(_)));
            bfs_results.insert(candidate, outcome);
            bfs_seconds.insert(candidate, seconds);
            if found {
                witnessed = true;
                break;
            }
        }
    }

    solve_query(
        opts.start,
        goal,
        index + 1,
        opts.goal_match,
        |candidate, transform| {
            let bfs_result = bfs_results.remove(&candidate);
            let (settled, seconds) = match (bfs_result, bfs_seconds.remove(&candidate)) {
                (Some(outcome), Some(seconds)) => (outcome, seconds),
                (None, None) => (None, 0.0),
                _ => return Err("reverse BFS result and timing disagree".to_owned()),
            };
            if witnessed && settled.is_none() {
                let outcome = ExactOutcome::Unknown;
                print_instance_verbose(
                    opts.verbose,
                    index + 1,
                    candidate,
                    "bfs",
                    0,
                    0,
                    0,
                    seconds,
                    0.0,
                    "skipped",
                );
                print_candidate_verbose(
                    opts.verbose,
                    index + 1,
                    candidate,
                    &outcome,
                    None,
                    seconds,
                );
                return Ok(Some(outcome));
            }
            check_exact_reachability(opts, candidate, index + 1, transform, settled, seconds)
        },
    )
}

/// Check a reverse-search witness against the candidate it must prove and
/// replay it with the ordinary rules before accepting it as a SAT outcome.
fn verified_witness(
    record: crate::record::ReachabilityRecord,
    table: &ForwardTable,
    goal: Position,
) -> Result<ExactOutcome, String> {
    if record.start != table.start() || record.goal != goal {
        return Err("reverse BFS witness is for another query".to_owned());
    }
    if record.verify()?.last() != Some(&goal) {
        return Err("reverse BFS witness does not reach the goal".to_owned());
    }
    Ok(ExactOutcome::Reachable(record.actions))
}

/// Run the bounded breadth-first reverse search.
fn run_bfs(
    goal: Position,
    table: &ForwardTable,
    bfs: ReverseBfs,
    input_index: usize,
    verbose: bool,
) -> Result<(Option<ExactOutcome>, f64), String> {
    let started = Instant::now();
    let outcome = retrospective_bfs(goal, table, bfs.state_budget)
        .map_err(|error| format!("reverse BFS failed: {error}"))?;
    let (result, nodes, result_name) = match outcome {
        BfsOutcome::Resolved(SearchOutcome::Found { record, nodes }) => {
            (Some(verified_witness(record, table, goal)?), nodes, "found")
        }
        BfsOutcome::Resolved(SearchOutcome::NotFound { nodes }) => {
            (Some(ExactOutcome::Unreachable), nodes, "exhausted")
        }
        BfsOutcome::Resolved(SearchOutcome::Unknown { nodes }) => (None, nodes, "unknown"),
        BfsOutcome::Frontier { nodes } => (None, nodes, "unknown"),
    };

    let seconds = started.elapsed().as_secs_f64();
    if verbose {
        println!(
            "[bfs] input={} goal={} result={} nodes={} secs={:.6}",
            input_index,
            format_position(goal),
            result_name,
            nodes,
            seconds,
        );
    }
    Ok((result, seconds))
}

fn check_exact_reachability(
    opts: &RunOptions,
    goal: Position,
    input_index: usize,
    transform: PositionTransform,
    settled: Option<ExactOutcome>,
    bfs_seconds: f64,
) -> Result<Option<ExactOutcome>, String> {
    let candidate_started = Instant::now();
    if let Some(outcome) = settled {
        print_instance_verbose(
            opts.verbose,
            input_index,
            goal,
            "bfs",
            0,
            0,
            0,
            bfs_seconds,
            0.0,
            "trivial",
        );
        print_candidate_verbose(
            opts.verbose,
            input_index,
            goal,
            &outcome,
            Some(("bfs", 0)),
            bfs_seconds + candidate_started.elapsed().as_secs_f64(),
        );
        return Ok(Some(outcome));
    }
    let mut built = Vec::with_capacity(opts.encodings.encodings().len());
    for &encoding in opts.encodings.encodings() {
        let build_started = Instant::now();
        let result = encode_goal(opts.start, goal, encoding)?;
        built.push(BuiltEncoding {
            encoding,
            build_seconds: build_started.elapsed().as_secs_f64(),
            result,
        });
    }

    for encoded in &built {
        if let Some(dir) = opts.cnf_dump_dir.as_deref() {
            let path = dir.join(format!(
                "goal_{input_index:04}_{transform}_{}.cnf",
                encoded.encoding
            ));
            match &encoded.result {
                EncodingResult::Trivial(outcome) => {
                    let clauses = trivial_clauses(encoded.encoding, outcome)?;
                    dump_dimacs_cnf(&path, 0, &clauses)?;
                }
                EncodingResult::Cnf(problem) => {
                    dump_dimacs_cnf(&path, problem.variable_count(), &problem.clauses)?;
                }
            }
        }
    }

    validate_trivial_results(&built)?;
    if opts.cnf_dump_only {
        for encoded in &built {
            let EncodingResult::Trivial(outcome) = &encoded.result else {
                continue;
            };
            let result = if matches!(outcome, ExactOutcome::Unknown) {
                "skipped"
            } else {
                "trivial"
            };
            print_instance_verbose(
                opts.verbose,
                input_index,
                goal,
                encoded.encoding.as_str(),
                0,
                0,
                0,
                encoded.build_seconds,
                0.0,
                result,
            );
        }
        return Ok(None);
    }

    if let Some((encoding, actions, build_seconds)) = built.iter().find_map(|encoded| {
        let EncodingResult::Trivial(ExactOutcome::Reachable(actions)) = &encoded.result else {
            return None;
        };
        Some((encoded.encoding, actions.clone(), encoded.build_seconds))
    }) {
        print_instance_verbose(
            opts.verbose,
            input_index,
            goal,
            encoding.as_str(),
            0,
            0,
            0,
            build_seconds,
            0.0,
            "trivial",
        );
        let outcome = ExactOutcome::Reachable(actions);
        print_candidate_verbose(
            opts.verbose,
            input_index,
            goal,
            &outcome,
            Some((encoding.as_str(), 0)),
            bfs_seconds + candidate_started.elapsed().as_secs_f64(),
        );
        return Ok(Some(outcome));
    }

    if let Some((encoding, build_seconds)) = built.iter().find_map(|encoded| {
        matches!(
            encoded.result,
            EncodingResult::Trivial(ExactOutcome::Unreachable)
        )
        .then_some((encoded.encoding, encoded.build_seconds))
    }) {
        print_instance_verbose(
            opts.verbose,
            input_index,
            goal,
            encoding.as_str(),
            0,
            0,
            0,
            build_seconds,
            0.0,
            "trivial",
        );
        let outcome = ExactOutcome::Unreachable;
        print_candidate_verbose(
            opts.verbose,
            input_index,
            goal,
            &outcome,
            Some((encoding.as_str(), 0)),
            bfs_seconds + candidate_started.elapsed().as_secs_f64(),
        );
        return Ok(Some(outcome));
    }

    for encoded in &built {
        if matches!(
            encoded.result,
            EncodingResult::Trivial(ExactOutcome::Unknown)
        ) {
            print_instance_verbose(
                opts.verbose,
                input_index,
                goal,
                encoded.encoding.as_str(),
                0,
                0,
                0,
                encoded.build_seconds,
                0.0,
                "skipped",
            );
        }
    }

    let problems: Vec<_> = built
        .iter()
        .filter_map(|encoded| match &encoded.result {
            EncodingResult::Cnf(problem) => Some(PortfolioProblem {
                encoding: encoded.encoding,
                problem,
            }),
            EncodingResult::Trivial(_) => None,
        })
        .collect();
    let solved = if problems.is_empty() {
        SolvedPortfolio {
            outcome: ExactOutcome::Unknown,
            decided_by: None,
            reports: Vec::new(),
        }
    } else {
        solve_portfolio(&problems, opts.seeds, opts.sat_timeout_per_instance)?
    };
    for report in &solved.reports {
        let encoded = built
            .iter()
            .find(|encoded| encoded.encoding == report.key.encoding)
            .expect("every solver report belongs to a built encoding");
        let EncodingResult::Cnf(problem) = &encoded.result else {
            unreachable!("only CNF encodings have solver reports");
        };
        if let Some(result) = report.result_name() {
            print_instance_verbose(
                opts.verbose,
                input_index,
                goal,
                report.key.encoding.as_str(),
                report.key.seed,
                problem.variable_count(),
                problem.clause_count(),
                encoded.build_seconds,
                report.solve_seconds,
                result,
            );
        }
    }
    print_candidate_verbose(
        opts.verbose,
        input_index,
        goal,
        &solved.outcome,
        solved
            .decided_by
            .map(|key| (key.encoding.as_str(), key.seed)),
        bfs_seconds + candidate_started.elapsed().as_secs_f64(),
    );
    Ok(Some(solved.outcome))
}

struct BuiltEncoding {
    encoding: SatEncoding,
    build_seconds: f64,
    result: EncodingResult,
}

fn encode_goal(
    start: Position,
    goal: Position,
    encoding: SatEncoding,
) -> Result<EncodingResult, String> {
    if encoding == SatEncoding::NoPassWitness {
        return nopass::encode(start, goal);
    }
    if let Some(outcome) = zero_placement_outcome(start, goal)? {
        return Ok(EncodingResult::Trivial(outcome));
    }
    match encoding {
        SatEncoding::Ply => ply::encode(start, goal),
        SatEncoding::Placement => placement::encode(start, goal, false),
        SatEncoding::PlacementColumns => placement::encode(start, goal, true),
        SatEncoding::PlacementRays => rays::encode(start, goal),
        SatEncoding::NoPassWitness => unreachable!("witness encoding handled separately"),
    }
}

fn validate_trivial_results(encodings: &[BuiltEncoding]) -> Result<(), String> {
    for encoded in encodings {
        match &encoded.result {
            EncodingResult::Trivial(ExactOutcome::Unreachable)
                if !encoded.encoding.proves_unreachable() =>
            {
                return Err("witness encoding returned an unreachability conclusion".to_owned());
            }
            EncodingResult::Trivial(ExactOutcome::Unknown)
                if encoded.encoding.proves_unreachable() =>
            {
                return Err("an encoding returned UNKNOWN without a solver call".to_owned());
            }
            _ => {}
        }
    }
    Ok(())
}

fn trivial_clauses(encoding: SatEncoding, outcome: &ExactOutcome) -> Result<Vec<Vec<i32>>, String> {
    match outcome {
        ExactOutcome::Reachable(_) => Ok(Vec::new()),
        ExactOutcome::Unreachable if encoding.proves_unreachable() => Ok(vec![Vec::new()]),
        ExactOutcome::Unreachable => {
            Err("witness encoding returned an unreachability conclusion".to_owned())
        }
        ExactOutcome::Unknown if !encoding.proves_unreachable() => Ok(vec![Vec::new()]),
        ExactOutcome::Unknown => {
            Err("an encoding returned UNKNOWN without a solver call".to_owned())
        }
    }
}

fn zero_placement_outcome(start: Position, goal: Position) -> Result<Option<ExactOutcome>, String> {
    if start.board.popcount() != goal.board.popcount() {
        return Ok(None);
    }
    let actions = if start == goal {
        Some(Vec::new())
    } else if get_moves(start.board.player, start.board.opponent) == 0
        && get_moves(start.board.opponent, start.board.player) != 0
        && goal
            == (Position {
                board: start.board.swapped(),
                side_to_move: start.side_to_move.opposite(),
            })
    {
        Some(vec![PlyAction::Pass])
    } else {
        None
    };
    let Some(actions) = actions else {
        return Ok(Some(ExactOutcome::Unreachable));
    };
    if replay(start, &actions)?.last() != Some(&goal) {
        return Err("zero-placement witness failed replay".to_owned());
    }
    Ok(Some(ExactOutcome::Reachable(actions)))
}

#[allow(clippy::too_many_arguments)]
fn print_instance_verbose(
    verbose: bool,
    input_index: usize,
    goal: Position,
    encoding: &str,
    seed: usize,
    variables: usize,
    clauses: usize,
    build_seconds: f64,
    solve_seconds: f64,
    result: &str,
) {
    if verbose {
        println!(
            "[sat] input={} goal={} encoding={} seed={} vars={} clauses={} build_secs={:.6} solve_secs={:.6} result={}",
            input_index,
            format_position(goal),
            encoding,
            seed,
            variables,
            clauses,
            build_seconds,
            solve_seconds,
            result,
        );
    }
}

fn print_candidate_verbose(
    verbose: bool,
    input_index: usize,
    goal: Position,
    outcome: &ExactOutcome,
    decided_by: Option<(&str, usize)>,
    seconds: f64,
) {
    if !verbose {
        return;
    }
    let outcome = match outcome {
        ExactOutcome::Reachable(_) => "reachable",
        ExactOutcome::Unreachable => "unreachable",
        ExactOutcome::Unknown => "unknown",
    };
    let decided_by = decided_by
        .map(|(encoding, seed)| format!("{encoding}#{seed}"))
        .unwrap_or_else(|| "none".to_owned());
    println!(
        "[sat] input={} goal={} outcome={} decided_by={} secs={:.6}",
        input_index,
        format_position(goal),
        outcome,
        decided_by,
        seconds,
    );
}

fn print_classification_verbose(
    verbose: bool,
    input_index: usize,
    outcome: &GoalClassification,
    seconds: f64,
) {
    if !verbose {
        return;
    }
    let classification = match outcome {
        GoalClassification::Reachable { .. } => "OK",
        GoalClassification::Unreachable { .. } => "NG",
        GoalClassification::Unknown { .. } => "UNKNOWN",
    };
    println!(
        "[sat] input={} classification={} secs={:.6}",
        input_index, classification, seconds,
    );
}

fn dump_dimacs_cnf(path: &Path, variable_count: usize, clauses: &[Vec<i32>]) -> Result<(), String> {
    let file = File::create(path)
        .map_err(|error| format!("failed to create CNF {}: {error}", path.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "p cnf {} {}", variable_count, clauses.len())
        .map_err(|error| format!("failed to write CNF header: {error}"))?;
    for clause in clauses {
        for literal in clause {
            write!(writer, "{literal} ")
                .map_err(|error| format!("failed to write CNF clause: {error}"))?;
        }
        writeln!(writer, "0").map_err(|error| format!("failed to write CNF clause: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("failed to flush CNF {}: {error}", path.display()))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn solve_problem(
    problem: &CnfProblem,
    timeout: Option<Duration>,
) -> Result<ExactOutcome, String> {
    let encoding = match problem.proof_scope {
        ProofScope::Complete => SatEncoding::Ply,
        ProofScope::WitnessOnly => SatEncoding::NoPassWitness,
    };
    let solved = solve_portfolio(&[PortfolioProblem { encoding, problem }], 1, timeout)?;
    Ok(solved.outcome)
}

#[derive(Clone, Copy)]
struct PortfolioProblem<'a> {
    encoding: SatEncoding,
    problem: &'a CnfProblem,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct WorkerKey {
    encoding: SatEncoding,
    seed: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StopReason {
    Decision,
    Deadline,
}

struct RegistryState {
    stop_reason: Option<StopReason>,
    interrupters: Vec<Option<Interrupter>>,
}

struct InterrupterRegistry {
    state: Mutex<RegistryState>,
}

impl InterrupterRegistry {
    fn new(workers: usize) -> Self {
        Self {
            state: Mutex::new(RegistryState {
                stop_reason: None,
                interrupters: (0..workers).map(|_| None).collect(),
            }),
        }
    }

    fn stop_all(&self, reason: StopReason) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "SAT interrupter registry was poisoned".to_owned())?;
        state.stop_reason.get_or_insert(reason);
        for interrupter in state.interrupters.iter().flatten() {
            interrupter.interrupt();
        }
        Ok(())
    }

    fn stop_reason(&self) -> Result<Option<StopReason>, String> {
        self.state
            .lock()
            .map(|state| state.stop_reason)
            .map_err(|_| "SAT interrupter registry was poisoned".to_owned())
    }
}

struct RegistryGuard<'a> {
    registry: &'a InterrupterRegistry,
    index: usize,
}

impl<'a> RegistryGuard<'a> {
    fn register(
        registry: &'a InterrupterRegistry,
        index: usize,
        interrupter: Interrupter,
    ) -> Result<(Self, Option<StopReason>), String> {
        let mut state = registry
            .state
            .lock()
            .map_err(|_| "SAT interrupter registry was poisoned".to_owned())?;
        let stop_reason = state.stop_reason;
        let slot = state
            .interrupters
            .get_mut(index)
            .ok_or_else(|| format!("invalid SAT worker registry index {index}"))?;
        if slot.is_some() {
            return Err(format!(
                "SAT worker registry index {index} is already occupied"
            ));
        }
        *slot = Some(interrupter);
        drop(state);
        Ok((Self { registry, index }, stop_reason))
    }
}

impl Drop for RegistryGuard<'_> {
    fn drop(&mut self) {
        let mut state = self
            .registry
            .state
            .lock()
            .expect("SAT interrupter registry was poisoned while unregistering");
        state.interrupters[self.index] = None;
    }
}

#[derive(Debug)]
enum WorkerEvent {
    Reachable(Vec<PlyAction>),
    CompleteUnsat,
    WitnessUnsat,
    Interrupted(StopReason),
    UnexpectedInterrupted,
    Error(String),
    Missing,
    Panicked,
}

#[derive(Debug)]
struct WorkerReport {
    key: WorkerKey,
    solve_seconds: f64,
    event: WorkerEvent,
}

impl WorkerReport {
    fn result_name(&self) -> Option<&'static str> {
        match self.event {
            WorkerEvent::Reachable(_) => Some("sat"),
            WorkerEvent::CompleteUnsat | WorkerEvent::WitnessUnsat => Some("unsat"),
            WorkerEvent::Interrupted(StopReason::Decision) => Some("interrupted"),
            WorkerEvent::Interrupted(StopReason::Deadline) => Some("timeout"),
            WorkerEvent::UnexpectedInterrupted
            | WorkerEvent::Error(_)
            | WorkerEvent::Missing
            | WorkerEvent::Panicked => None,
        }
    }

    fn requests_stop(&self) -> bool {
        matches!(
            self.event,
            WorkerEvent::Reachable(_)
                | WorkerEvent::CompleteUnsat
                | WorkerEvent::UnexpectedInterrupted
                | WorkerEvent::Error(_)
                | WorkerEvent::Panicked
        )
    }
}

struct SolvedPortfolio {
    outcome: ExactOutcome,
    decided_by: Option<WorkerKey>,
    reports: Vec<WorkerReport>,
}

fn solve_portfolio(
    problems: &[PortfolioProblem<'_>],
    seeds: usize,
    timeout: Option<Duration>,
) -> Result<SolvedPortfolio, String> {
    if seeds == 0 {
        return Err("seeds must be positive".to_owned());
    }
    let worker_count = problems
        .len()
        .checked_mul(seeds)
        .ok_or_else(|| "SAT portfolio worker count overflow".to_owned())?;
    if worker_count == 0 {
        return Ok(SolvedPortfolio {
            outcome: ExactOutcome::Unknown,
            decided_by: None,
            reports: Vec::new(),
        });
    }

    let registry = InterrupterRegistry::new(worker_count);
    let (sender, receiver) = mpsc::channel();
    let expected: Vec<_> = problems
        .iter()
        .flat_map(|problem| {
            (0..seeds).map(move |seed| WorkerKey {
                encoding: problem.encoding,
                seed,
            })
        })
        .collect();
    let solve_started = Instant::now();
    let deadline = match timeout {
        Some(limit) => Some(
            solve_started
                .checked_add(limit)
                .ok_or_else(|| "SAT deadline overflow".to_owned())?,
        ),
        None => None,
    };
    let mut reports = Vec::with_capacity(worker_count);
    let mut collection_error = None;

    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for (problem_index, &problem) in problems.iter().enumerate() {
            for seed in 0..seeds {
                let worker_index = problem_index * seeds + seed;
                let key = WorkerKey {
                    encoding: problem.encoding,
                    seed,
                };
                let sender = sender.clone();
                let registry = &registry;
                handles.push((
                    key,
                    scope.spawn(move || {
                        let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            solve_worker(worker_index, key, problem, registry)
                        }))
                        .unwrap_or(WorkerReport {
                            key,
                            solve_seconds: 0.0,
                            event: WorkerEvent::Panicked,
                        });
                        let _ = sender.send(report);
                    }),
                ));
            }
        }
        drop(sender);

        let mut deadline_reached = false;
        while reports.len() < worker_count {
            let received = if deadline_reached {
                receiver
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            } else if let Some(deadline) = deadline {
                match deadline.checked_duration_since(Instant::now()) {
                    Some(remaining) => receiver.recv_timeout(remaining),
                    None => Err(mpsc::RecvTimeoutError::Timeout),
                }
            } else {
                receiver
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            };
            match received {
                Ok(report) => {
                    if report.requests_stop() {
                        if let Err(error) = registry.stop_all(StopReason::Decision) {
                            if collection_error.is_none() {
                                collection_error = Some(error);
                            }
                        }
                    }
                    reports.push(report);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    deadline_reached = true;
                    if let Err(error) = registry.stop_all(StopReason::Deadline) {
                        if collection_error.is_none() {
                            collection_error = Some(error);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        for (key, handle) in handles {
            if handle.join().is_err() {
                reports.push(WorkerReport {
                    key,
                    solve_seconds: 0.0,
                    event: WorkerEvent::Panicked,
                });
            }
        }
    });

    for &key in &expected {
        if !reports.iter().any(|report| report.key == key) {
            reports.push(WorkerReport {
                key,
                solve_seconds: 0.0,
                event: WorkerEvent::Missing,
            });
        }
    }
    if let Some(error) = collection_error {
        return Err(error);
    }
    let (outcome, decided_by) = combine_reports(&reports)?;
    Ok(SolvedPortfolio {
        outcome,
        decided_by,
        reports,
    })
}

fn solve_worker(
    worker_index: usize,
    key: WorkerKey,
    problem: PortfolioProblem<'_>,
    registry: &InterrupterRegistry,
) -> WorkerReport {
    let mut solver = rustsat_kissat::Kissat::default();
    if let Err(error) = configure_seed(&mut solver, key.seed) {
        return worker_error(key, error);
    }
    if let Err(error) = add_problem_to_solver(&mut solver, problem.problem) {
        return worker_error(key, error);
    }
    let solve_started = Instant::now();
    let (solve_result, interruption_reason) = {
        let interrupter = solver.interrupter();
        // This guard must be declared after the solver. Rust drops locals in
        // reverse declaration order, so every exit unregisters the raw-pointer
        // interrupter before Kissat releases the allocation it points into.
        let (_registry_guard, stop_reason) =
            match RegistryGuard::register(registry, worker_index, interrupter) {
                Ok(registration) => registration,
                Err(error) => return worker_error(key, error),
            };
        if let Some(reason) = stop_reason {
            return WorkerReport {
                key,
                solve_seconds: 0.0,
                event: WorkerEvent::Interrupted(reason),
            };
        }
        let result = solver.solve();
        let interruption_reason = if matches!(&result, Ok(SolverResult::Interrupted)) {
            match registry.stop_reason() {
                Ok(reason) => reason,
                Err(error) => return worker_error(key, error),
            }
        } else {
            None
        };
        (result, interruption_reason)
    };
    let event = match solve_result {
        Ok(SolverResult::Sat) => match problem.problem.decoder.decode_and_verify(&solver) {
            Ok(actions) => WorkerEvent::Reachable(actions),
            Err(error) => WorkerEvent::Error(error),
        },
        Ok(SolverResult::Unsat) => match problem.problem.proof_scope {
            ProofScope::Complete => WorkerEvent::CompleteUnsat,
            ProofScope::WitnessOnly => WorkerEvent::WitnessUnsat,
        },
        Ok(SolverResult::Interrupted) => {
            interruption_reason.map_or(WorkerEvent::UnexpectedInterrupted, WorkerEvent::Interrupted)
        }
        Err(error) => WorkerEvent::Error(format!("kissat failed: {error}")),
    };
    WorkerReport {
        key,
        solve_seconds: solve_started.elapsed().as_secs_f64(),
        event,
    }
}

fn worker_error(key: WorkerKey, error: String) -> WorkerReport {
    WorkerReport {
        key,
        solve_seconds: 0.0,
        event: WorkerEvent::Error(error),
    }
}

fn configure_seed(solver: &mut rustsat_kissat::Kissat<'_>, seed: usize) -> Result<(), String> {
    let seed = i32::try_from(seed).map_err(|_| "SAT seed exceeds Kissat's range".to_owned())?;
    if seed != 0 {
        // rustsat-kissat 0.7.4 treats Kissat's previous option value as a
        // success flag. The initial value is zero, so the first valid update is
        // reported as an error. Ignore that Result and verify by readback.
        let _ = solver.set_option("seed", seed);
    }
    let actual = solver
        .get_option("seed")
        .map_err(|error| format!("failed to read Kissat seed: {error}"))?;
    if actual != seed {
        return Err(format!(
            "failed to set Kissat seed: requested {seed}, read back {actual}"
        ));
    }
    Ok(())
}

fn add_problem_to_solver(
    solver: &mut rustsat_kissat::Kissat<'_>,
    problem: &CnfProblem,
) -> Result<(), String> {
    let mut cnf = Cnf::new();
    for raw_clause in &problem.clauses {
        let mut clause = Clause::new();
        for &literal in raw_clause {
            clause.add(cnf::lit(literal)?);
        }
        cnf.add_clause(clause);
    }
    solver
        .add_cnf(cnf)
        .map_err(|error| format!("failed to add CNF to Kissat: {error}"))
}

fn combine_reports(reports: &[WorkerReport]) -> Result<(ExactOutcome, Option<WorkerKey>), String> {
    let mut seen = Vec::with_capacity(reports.len());
    let mut tentative = None;
    let mut saw_reachable = false;
    let mut saw_complete_unsat = false;
    let mut first_error = None;

    for report in reports {
        if !seen.contains(&report.key) {
            seen.push(report.key);
        } else if first_error.is_none() {
            first_error = Some(format!(
                "SAT worker {}#{} reported more than once",
                report.key.encoding, report.key.seed
            ));
        }
        match &report.event {
            WorkerEvent::Reachable(actions) => {
                if saw_complete_unsat && first_error.is_none() {
                    first_error = Some(
                        "verified SAT witness contradicts a complete-scope UNSAT result".to_owned(),
                    );
                }
                saw_reachable = true;
                if tentative.is_none() {
                    tentative = Some((ExactOutcome::Reachable(actions.clone()), Some(report.key)));
                }
            }
            WorkerEvent::CompleteUnsat => {
                if saw_reachable && first_error.is_none() {
                    first_error = Some(
                        "complete-scope UNSAT result contradicts a verified SAT witness".to_owned(),
                    );
                }
                saw_complete_unsat = true;
                if tentative.is_none() {
                    tentative = Some((ExactOutcome::Unreachable, Some(report.key)));
                }
            }
            WorkerEvent::WitnessUnsat | WorkerEvent::Interrupted(_) => {}
            WorkerEvent::UnexpectedInterrupted if first_error.is_none() => {
                first_error = Some(format!(
                    "SAT worker {}#{} was interrupted without a stop request or deadline",
                    report.key.encoding, report.key.seed
                ));
            }
            WorkerEvent::Error(error) if first_error.is_none() => {
                first_error = Some(format!(
                    "SAT worker {}#{} failed: {error}",
                    report.key.encoding, report.key.seed
                ));
            }
            WorkerEvent::Missing if first_error.is_none() => {
                first_error = Some(format!(
                    "SAT worker {}#{} did not report",
                    report.key.encoding, report.key.seed
                ));
            }
            WorkerEvent::Panicked if first_error.is_none() => {
                first_error = Some(format!(
                    "SAT worker {}#{} panicked",
                    report.key.encoding, report.key.seed
                ));
            }
            WorkerEvent::UnexpectedInterrupted
            | WorkerEvent::Error(_)
            | WorkerEvent::Missing
            | WorkerEvent::Panicked => {}
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(tentative.unwrap_or((ExactOutcome::Unknown, None)))
}

fn print_result(
    outcome: &GoalClassification,
    show_coords: bool,
    show_boards: bool,
) -> Result<(), String> {
    match outcome {
        GoalClassification::Reachable { record } => {
            println!(
                "[{}] SAT (H={}, transform={})",
                record.input_index,
                record.actions.len(),
                record.transform
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::othello::{Board, Color};

    #[test]
    fn encoding_names_round_trip() {
        for encoding in SatEncoding::ALL {
            assert_eq!(encoding.as_str().parse(), Ok(encoding));
            assert_eq!(encoding.to_string(), encoding.as_str());
        }
        assert!("unknown".parse::<SatEncoding>().is_err());
    }

    #[test]
    fn portfolio_names_round_trip_and_reject_invalid_lists() {
        for encoding in SatEncoding::ALL {
            let portfolio = SatPortfolio::from(encoding);
            assert_eq!(portfolio.encodings(), &[encoding]);
            assert_eq!(portfolio.to_string(), encoding.as_str());
            assert_eq!(portfolio.to_string().parse(), Ok(portfolio));
        }

        let portfolio = SatPortfolio::new(vec![
            SatEncoding::NoPassWitness,
            SatEncoding::PlacementColumns,
        ])
        .unwrap();
        assert_eq!(portfolio.to_string(), "nopass-witness+placement-columns");
        assert_eq!(portfolio.to_string().parse(), Ok(portfolio.clone()));
        assert!(portfolio.proves_unreachable());
        assert!(!SatPortfolio::from(SatEncoding::NoPassWitness).proves_unreachable());

        assert!(SatPortfolio::new(Vec::new()).is_err());
        for text in [
            "",
            "unknown",
            "ply+ply",
            "+ply",
            "ply+",
            "nopass-witness+unknown",
        ] {
            assert!(text.parse::<SatPortfolio>().is_err(), "{text}");
        }
    }

    #[test]
    fn kissat_seed_is_verified_by_readback() {
        for seed in 0..=2 {
            let mut solver = rustsat_kissat::Kissat::default();
            configure_seed(&mut solver, seed).unwrap();
            assert_eq!(solver.get_option("seed").unwrap(), seed as i32);
        }
    }

    fn report(encoding: SatEncoding, seed: usize, event: WorkerEvent) -> WorkerReport {
        WorkerReport {
            key: WorkerKey { encoding, seed },
            solve_seconds: 0.0,
            event,
        }
    }

    #[test]
    fn report_combination_uses_the_first_verified_witness() {
        let reports = [
            report(SatEncoding::NoPassWitness, 0, WorkerEvent::WitnessUnsat),
            report(
                SatEncoding::PlacementColumns,
                0,
                WorkerEvent::Reachable(vec![PlyAction::Play(19)]),
            ),
            report(
                SatEncoding::PlacementColumns,
                1,
                WorkerEvent::Reachable(vec![PlyAction::Play(26)]),
            ),
            report(
                SatEncoding::NoPassWitness,
                1,
                WorkerEvent::Interrupted(StopReason::Decision),
            ),
        ];
        let (outcome, decided_by) = combine_reports(&reports).unwrap();
        assert!(matches!(
            outcome,
            ExactOutcome::Reachable(actions) if actions == vec![PlyAction::Play(19)]
        ));
        assert_eq!(
            decided_by,
            Some(WorkerKey {
                encoding: SatEncoding::PlacementColumns,
                seed: 0,
            })
        );
    }

    #[test]
    fn report_combination_keeps_a_tentative_decision_after_deadline() {
        let reports = [
            report(SatEncoding::PlacementColumns, 1, WorkerEvent::CompleteUnsat),
            report(
                SatEncoding::NoPassWitness,
                0,
                WorkerEvent::Interrupted(StopReason::Deadline),
            ),
        ];
        let (outcome, decided_by) = combine_reports(&reports).unwrap();
        assert!(matches!(outcome, ExactOutcome::Unreachable));
        assert_eq!(decided_by, Some(reports[0].key));

        let witness_only = [report(
            SatEncoding::NoPassWitness,
            0,
            WorkerEvent::WitnessUnsat,
        )];
        let (outcome, decided_by) = combine_reports(&witness_only).unwrap();
        assert!(matches!(outcome, ExactOutcome::Unknown));
        assert_eq!(decided_by, None);
    }

    #[test]
    fn report_combination_rejects_errors_and_contradictions_in_any_order() {
        for reports in [
            vec![
                report(
                    SatEncoding::PlacementColumns,
                    0,
                    WorkerEvent::Reachable(Vec::new()),
                ),
                report(SatEncoding::PlacementColumns, 1, WorkerEvent::CompleteUnsat),
            ],
            vec![
                report(SatEncoding::PlacementColumns, 0, WorkerEvent::CompleteUnsat),
                report(
                    SatEncoding::PlacementColumns,
                    1,
                    WorkerEvent::Reachable(Vec::new()),
                ),
            ],
            vec![
                report(SatEncoding::PlacementColumns, 0, WorkerEvent::CompleteUnsat),
                report(
                    SatEncoding::NoPassWitness,
                    0,
                    WorkerEvent::Error("decoder failed".to_owned()),
                ),
            ],
        ] {
            assert!(combine_reports(&reports).is_err());
        }
    }

    #[test]
    fn report_combination_rejects_unexpected_interrupts_missing_reports_and_panics() {
        for event in [
            WorkerEvent::UnexpectedInterrupted,
            WorkerEvent::Missing,
            WorkerEvent::Panicked,
        ] {
            assert!(combine_reports(&[report(SatEncoding::Ply, 0, event)]).is_err());
        }
    }

    #[test]
    fn zero_placement_decision_accepts_empty_and_forced_pass_witnesses() {
        let initial = Position::initial();
        assert!(matches!(
            zero_placement_outcome(initial, initial).unwrap(),
            Some(ExactOutcome::Reachable(actions)) if actions.is_empty()
        ));

        let forced_pass = Position {
            board: Board::new(1_u64 << 1, !3_u64),
            side_to_move: Color::White,
        };
        let goal = *replay(forced_pass, &[PlyAction::Pass])
            .unwrap()
            .last()
            .unwrap();
        assert!(matches!(
            zero_placement_outcome(forced_pass, goal).unwrap(),
            Some(ExactOutcome::Reachable(actions)) if actions == vec![PlyAction::Pass]
        ));
    }

    #[test]
    fn zero_placement_decision_rejects_other_equal_count_goals() {
        let initial = Position::initial();
        let opposite_turn = Position {
            board: initial.board.swapped(),
            side_to_move: initial.side_to_move.opposite(),
        };
        assert!(matches!(
            zero_placement_outcome(initial, opposite_turn).unwrap(),
            Some(ExactOutcome::Unreachable)
        ));

        let terminal = Position {
            board: Board::new(u64::MAX, 0),
            side_to_move: Color::Black,
        };
        let after_terminal = Position {
            board: terminal.board.swapped(),
            side_to_move: terminal.side_to_move.opposite(),
        };
        assert!(matches!(
            zero_placement_outcome(terminal, after_terminal).unwrap(),
            Some(ExactOutcome::Unreachable)
        ));

        let one_placement = *replay(initial, &[PlyAction::Play(19)])
            .unwrap()
            .last()
            .unwrap();
        assert!(zero_placement_outcome(initial, one_placement)
            .unwrap()
            .is_none());
    }
}
