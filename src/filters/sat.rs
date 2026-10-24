//! Exact reachability through selectable SAT encodings.

use crate::filters::{layer_sat, placement_sat};
use crate::io::format_position;
use crate::othello::{get_moves, validate_board, Position};
use crate::query::{solve_query, ExactOutcome, GoalClassification};
use crate::record::{format_actions, replay, GoalMatch, PlyAction};
use rayon::{prelude::*, ThreadPoolBuilder};
use rustsat::{
    instances::Cnf,
    solvers::{Interrupt, InterruptSolver, Solve, SolverResult},
    types::{Clause, Lit},
};
use std::fmt;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SatEncoding {
    Ply,
    Placement,
    PlacementColumns,
}

impl SatEncoding {
    pub const ALL: [Self; 3] = [Self::Ply, Self::Placement, Self::PlacementColumns];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ply => "ply",
            Self::Placement => "placement",
            Self::PlacementColumns => "placement-columns",
        }
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

/// Each goal is a relative board together with its absolute side to move.
/// Time limits apply separately to each candidate's solver call, not to a query.
/// CNF-only runs return no classifications and never call the outcome callback.
pub struct RunOptions {
    pub start: Position,
    pub goals: Vec<Position>,
    pub goal_match: GoalMatch,
    pub encoding: SatEncoding,
    pub parallel_goals: usize,
    pub show_coords: bool,
    pub show_boards: bool,
    pub verbose: bool,
    pub cnf_dump_dir: Option<PathBuf>,
    pub cnf_dump_only: bool,
    pub sat_timeout_per_instance: Option<Duration>,
}

pub(crate) trait ModelDecoder {
    fn decode_and_verify(
        &self,
        solver: &rustsat_kissat::Kissat<'_>,
    ) -> Result<Vec<PlyAction>, String>;
}

pub(crate) struct CnfProblem {
    variable_count: usize,
    clauses: Vec<Vec<i32>>,
    decoder: Box<dyn ModelDecoder>,
}

impl CnfProblem {
    pub(crate) fn new(
        variable_count: usize,
        clauses: Vec<Vec<i32>>,
        decoder: impl ModelDecoder + 'static,
    ) -> Self {
        Self {
            variable_count,
            clauses,
            decoder: Box::new(decoder),
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
    for (index, goal) in opts.goals.iter().enumerate() {
        validate_board(&goal.board)
            .map_err(|err| format!("invalid goal[{}]: {err:?}", index + 1))?;
    }
    if opts.parallel_goals == 0 {
        return Err("parallel_goals must be positive".to_owned());
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
    if let Some(dir) = &opts.cnf_dump_dir {
        fs::create_dir_all(dir).map_err(|error| {
            format!("failed to create CNF directory {}: {error}", dir.display())
        })?;
    }

    if opts.goals.is_empty() {
        return Ok(Vec::new());
    }
    let cancelled = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel();
    let pool = ThreadPoolBuilder::new()
        .num_threads(opts.parallel_goals.min(opts.goals.len()))
        .build()
        .map_err(|error| format!("failed to build rayon pool for SAT: {error}"))?;

    std::thread::scope(|scope| {
        let opts = &opts;
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
                            |candidate, transform| {
                                let path = opts.cnf_dump_dir.as_ref().map(|dir| {
                                    dir.join(format!("goal_{:04}_{}.cnf", index + 1, transform))
                                });
                                check_exact_reachability(
                                    opts.start,
                                    candidate,
                                    opts.encoding,
                                    path.as_deref(),
                                    opts.cnf_dump_only,
                                    opts.sat_timeout_per_instance,
                                    opts.verbose,
                                )
                            },
                        )
                        .map(|outcome| (index, outcome))
                        .map_err(|error| {
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

fn check_exact_reachability(
    start: Position,
    goal: Position,
    encoding: SatEncoding,
    cnf_path: Option<&Path>,
    cnf_dump_only: bool,
    timeout: Option<Duration>,
    verbose: bool,
) -> Result<Option<ExactOutcome>, String> {
    let build_started = Instant::now();
    let encoded = if let Some(outcome) = zero_placement_outcome(start, goal)? {
        EncodingResult::Trivial(outcome)
    } else {
        match encoding {
            SatEncoding::Ply => layer_sat::encode(start, goal),
            SatEncoding::Placement => placement_sat::encode(start, goal, false),
            SatEncoding::PlacementColumns => placement_sat::encode(start, goal, true),
        }?
    };
    let build_seconds = build_started.elapsed().as_secs_f64();

    match encoded {
        EncodingResult::Trivial(outcome) => {
            let clauses = match &outcome {
                ExactOutcome::Reachable(_) => Vec::new(),
                ExactOutcome::Unreachable => vec![Vec::new()],
                ExactOutcome::Unknown => {
                    return Err("an encoding returned UNKNOWN without a solver call".to_owned());
                }
            };
            if let Some(path) = cnf_path {
                dump_dimacs_cnf(path, 0, &clauses)?;
            }
            print_verbose(
                verbose,
                encoding,
                goal,
                0,
                clauses.len(),
                build_seconds,
                0.0,
            );
            Ok((!cnf_dump_only).then_some(outcome))
        }
        EncodingResult::Cnf(problem) => {
            if let Some(path) = cnf_path {
                dump_dimacs_cnf(path, problem.variable_count(), &problem.clauses)?;
            }
            if cnf_dump_only {
                print_verbose(
                    verbose,
                    encoding,
                    goal,
                    problem.variable_count(),
                    problem.clause_count(),
                    build_seconds,
                    0.0,
                );
                return Ok(None);
            }
            let solve_started = Instant::now();
            let outcome = solve_problem(&problem, timeout)?;
            print_verbose(
                verbose,
                encoding,
                goal,
                problem.variable_count(),
                problem.clause_count(),
                build_seconds,
                solve_started.elapsed().as_secs_f64(),
            );
            Ok(Some(outcome))
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

fn print_verbose(
    verbose: bool,
    encoding: SatEncoding,
    goal: Position,
    variables: usize,
    clauses: usize,
    build_seconds: f64,
    solve_seconds: f64,
) {
    if verbose {
        println!(
            "[sat] encoding={} goal={} vars={} clauses={} build_secs={:.6} solve_secs={:.6}",
            encoding,
            format_position(goal),
            variables,
            clauses,
            build_seconds,
            solve_seconds
        );
    }
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

pub(crate) fn solve_problem(
    problem: &CnfProblem,
    timeout: Option<Duration>,
) -> Result<ExactOutcome, String> {
    let mut solver = rustsat_kissat::Kissat::default();
    let mut cnf = Cnf::new();
    for raw_clause in &problem.clauses {
        let mut clause = Clause::new();
        for &literal in raw_clause {
            clause.add(to_rustsat_lit(literal)?);
        }
        cnf.add_clause(clause);
    }
    solver
        .add_cnf(cnf)
        .map_err(|error| format!("failed to add CNF to Kissat: {error}"))?;

    let timeout_guard = timeout.map(|limit| {
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
            .map_err(|_| "SAT timeout worker panicked".to_owned())?;
    }

    match solve_result.map_err(|error| format!("kissat failed: {error}"))? {
        SolverResult::Sat => Ok(ExactOutcome::Reachable(
            problem.decoder.decode_and_verify(&solver)?,
        )),
        SolverResult::Unsat => Ok(ExactOutcome::Unreachable),
        SolverResult::Interrupted if timeout.is_some() => Ok(ExactOutcome::Unknown),
        SolverResult::Interrupted => Err("kissat solve was interrupted".to_owned()),
    }
}

fn to_rustsat_lit(raw_literal: i32) -> Result<Lit, String> {
    if raw_literal > 0 {
        Ok(Lit::positive(raw_literal as u32))
    } else if raw_literal < 0 {
        Ok(Lit::negative((-raw_literal) as u32))
    } else {
        Err("CNF contains literal 0".to_owned())
    }
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
        for name in [
            "unknown",
            "ply-target",
            "placement-target",
            "placement-target-columns",
        ] {
            assert!(name.parse::<SatEncoding>().is_err(), "{name}");
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
