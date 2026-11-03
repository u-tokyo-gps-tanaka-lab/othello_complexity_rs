use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering},
    mpsc,
};

use serde::{Deserialize, Serialize};

use crate::io::{ensure_tri_outputs, format_position, TriCategory, TriOutcome, TriOutputs};
use crate::othello::{validate_board, Position};
use crate::query::{solve_query, ExactOutcome, GoalClassification};
use crate::record::GoalMatch;
use crate::search::{
    core::SearchOutcome,
    dfs::retrospective_search,
    external_bfs::{
        parallel_retrospective_bfs, sequential_retrospective_bfs, sync_parent,
        validate_fresh_directory, Cfg,
    },
    forward::ForwardTable,
    inmemory_bfs::parallel_inmemory_retrospective_bfs,
    move_ordering::retrospective_search_move_ordering,
    parallel_dfs::retrospective_search_parallel,
    parallel_gbfs::parallel_retrospective_greedy_best_first_search,
    transposition::Btable,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ExternalBfs {
    pub discs: u32,
    pub jobs: usize,
    pub verbose: u8,
    pub block_size: usize,
    pub tmp_dir: PathBuf,
    pub resume: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum Strategy {
    Dfs {
        discs: u32,
        max_nodes: usize,
    },
    DfsMoveOrdering {
        discs: u32,
        max_nodes: usize,
    },
    DfsParallel {
        discs: u32,
        max_nodes: usize,
        table_size: usize,
        threads: usize,
    },
    GbfsParallel {
        discs: u32,
        max_nodes: usize,
        use_lp: bool,
        threads: usize,
        parallel_goals: usize,
    },
    BfsExternal(ExternalBfs),
    BfsExternalParallel(ExternalBfs),
    BfsInmemoryParallel {
        discs: u32,
        max_nodes: usize,
        use_lp: bool,
        use_occupancy_cache: bool,
        threads: usize,
    },
}

impl Strategy {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Dfs { .. } => "dfs",
            Self::DfsMoveOrdering { .. } => "dfs-move-ordering",
            Self::DfsParallel { .. } => "dfs-parallel",
            Self::GbfsParallel { .. } => "gbfs-parallel",
            Self::BfsExternal(_) => "bfs-external",
            Self::BfsExternalParallel(_) => "bfs-external-parallel",
            Self::BfsInmemoryParallel { .. } => "bfs-inmemory-parallel",
        }
    }

    pub fn validate(&self) -> io::Result<()> {
        let discs = match self {
            Self::Dfs { discs, .. }
            | Self::DfsMoveOrdering { discs, .. }
            | Self::DfsParallel { discs, .. }
            | Self::GbfsParallel { discs, .. }
            | Self::BfsInmemoryParallel { discs, .. } => *discs,
            Self::BfsExternal(config) | Self::BfsExternalParallel(config) => config.discs,
        };
        if !(4..=64).contains(&discs) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "discs must be between 4 and 64",
            ));
        }
        if matches!(
            self,
            Self::BfsExternal(config) | Self::BfsExternalParallel(config)
                if config.block_size == 0
        ) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "block size must be positive",
            ));
        }
        if let Self::GbfsParallel {
            threads,
            parallel_goals,
            ..
        } = self
        {
            if *parallel_goals == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "parallel goals must be positive",
                ));
            }
            if *parallel_goals >= 2 && *threads == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "threads must be positive when parallel goals is at least 2",
                ));
            }
            if *parallel_goals >= 2 && *threads > 64 / *parallel_goals {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "parallel goals times threads must not exceed 64",
                ));
            }
        }
        Ok(())
    }
}

fn classify_position(
    input_index: usize,
    position: Position,
    discs: u32,
    goal_match: GoalMatch,
    table_pool: Option<&rayon::ThreadPool>,
    mut solve: impl FnMut(usize, Position, &ForwardTable) -> io::Result<SearchOutcome> + Send,
) -> io::Result<(GoalClassification, usize)> {
    if goal_match == GoalMatch::EquivalenceClass {
        if input_index == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "input index must be at least 1",
            ));
        }
        let mut search = || {
            let table = ForwardTable::for_equivalence(discs, position)?;
            solve(input_index, position, &table)
        };
        let outcome = match table_pool {
            Some(pool) => pool.install(search),
            None => search(),
        }?;
        let nodes = outcome.nodes();
        let classification = match outcome {
            SearchOutcome::Found { mut record, .. } => {
                if record.start != Position::initial()
                    || record.goal != position
                    || record.goal_match != GoalMatch::EquivalenceClass
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "equivalence search returned a record for another query",
                    ));
                }
                record.input_index = input_index;
                record
                    .verify()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                GoalClassification::Reachable { record }
            }
            SearchOutcome::NotFound { .. } => GoalClassification::Unreachable {
                input_index,
                goal: position,
                goal_match,
            },
            SearchOutcome::Unknown { .. } => GoalClassification::Unknown {
                input_index,
                goal: position,
                goal_match,
            },
        };
        return Ok((classification, nodes));
    }

    let mut nodes = 0_usize;
    let outcome = solve_query(
        Position::initial(),
        position,
        input_index,
        goal_match,
        |candidate, _transform| {
            let mut search = || {
                let table = ForwardTable::new(discs, Some(candidate))?;
                solve(input_index, candidate, &table)
            };
            let outcome = match table_pool {
                Some(pool) => pool.install(search),
                None => search(),
            }
            .map_err(|error| error.to_string())?;
            nodes = nodes
                .checked_add(outcome.nodes())
                .ok_or("reverse query node count overflow")?;
            Ok(Some(match outcome {
                SearchOutcome::Found { record, .. } => ExactOutcome::Reachable(record.actions),
                SearchOutcome::NotFound { .. } => ExactOutcome::Unreachable,
                SearchOutcome::Unknown { .. } => ExactOutcome::Unknown,
            }))
        },
    )
    .map_err(io::Error::other)?
    .ok_or_else(|| io::Error::other("reverse query produced no classification"))?;
    Ok((outcome, nodes))
}

fn write_query_result(
    outputs: &mut TriOutputs,
    records: &mut BufWriter<File>,
    result: &(GoalClassification, usize),
) -> io::Result<()> {
    let (outcome, nodes) = result;
    if let Some(record) = outcome.record() {
        writeln!(records, "{}", record.to_csv())?;
        records.flush()?;
    }
    let line = format_position(outcome.goal());
    outputs.write_result(outcome, &line)?;
    outputs.flush()?;
    let category = match outcome.category() {
        TriCategory::Ok => "OK",
        TriCategory::Ng => "NG",
        TriCategory::Unknown => "UNKNOWN",
    };
    println!("input {}: {category} nodes={nodes}", outcome.input_index());
    Ok(())
}

fn run_queries(
    positions: &[Position],
    out_dir: &Path,
    discs: u32,
    goal_match: GoalMatch,
    mut solve: impl FnMut(usize, Position, &ForwardTable) -> io::Result<SearchOutcome> + Send,
) -> io::Result<Vec<(GoalClassification, usize)>> {
    let prefix = format!("reverse_{goal_match}");
    let mut outputs = ensure_tri_outputs(out_dir, &prefix)?;
    let mut records = BufWriter::new(File::create(out_dir.join(format!("{prefix}_records.csv")))?);
    let mut results = Vec::with_capacity(positions.len());
    for (index, position) in positions.iter().copied().enumerate() {
        let result = classify_position(index + 1, position, discs, goal_match, None, &mut solve)?;
        write_query_result(&mut outputs, &mut records, &result)?;
        results.push(result);
    }
    records.flush()?;
    outputs.flush()?;
    Ok(results)
}

fn run_parallel_queries<F>(
    positions: &[Position],
    out_dir: &Path,
    discs: u32,
    goal_match: GoalMatch,
    parallel_goals: usize,
    threads: usize,
    solve: F,
) -> io::Result<Vec<(GoalClassification, usize)>>
where
    F: Fn(usize, Position, &ForwardTable) -> io::Result<SearchOutcome> + Sync,
{
    let prefix = format!("reverse_{goal_match}");
    let mut outputs = ensure_tri_outputs(out_dir, &prefix)?;
    let mut records = BufWriter::new(File::create(out_dir.join(format!("{prefix}_records.csv")))?);
    if positions.is_empty() {
        records.flush()?;
        outputs.flush()?;
        return Ok(Vec::new());
    }

    let cancelled = AtomicBool::new(false);
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();
    let workers = parallel_goals.min(positions.len());
    let mut results: Vec<Option<(GoalClassification, usize)>> = std::iter::repeat_with(|| None)
        .take(positions.len())
        .collect();
    let mut first_error = None;

    std::thread::scope(|scope| {
        let worker_cancelled = &cancelled;
        let worker_next = &next;
        let worker_solve = &solve;
        for _ in 0..workers {
            let tx = tx.clone();
            scope.spawn(move || {
                let pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
                    Ok(pool) => pool,
                    Err(error) => {
                        worker_cancelled.store(true, AtomicOrdering::Release);
                        let _ = tx.send(Err(io::Error::other(format!(
                            "failed to build rayon pool for a reverse-search worker: {error}"
                        ))));
                        return;
                    }
                };
                loop {
                    if worker_cancelled.load(AtomicOrdering::Acquire) {
                        return;
                    }
                    let index = worker_next.fetch_add(1, AtomicOrdering::Relaxed);
                    let Some(&position) = positions.get(index) else {
                        return;
                    };
                    if worker_cancelled.load(AtomicOrdering::Acquire) {
                        return;
                    }
                    let result = classify_position(
                        index + 1,
                        position,
                        discs,
                        goal_match,
                        Some(&pool),
                        worker_solve,
                    )
                    .map(|result| (index, result));
                    if result.is_err() {
                        worker_cancelled.store(true, AtomicOrdering::Release);
                    }
                    if tx.send(result).is_err() {
                        worker_cancelled.store(true, AtomicOrdering::Release);
                        return;
                    }
                }
            });
        }
        drop(tx);

        for received in rx {
            match received {
                Ok((index, result)) if first_error.is_none() => {
                    if let Err(error) = write_query_result(&mut outputs, &mut records, &result) {
                        cancelled.store(true, AtomicOrdering::Release);
                        first_error = Some(error);
                    } else {
                        results[index] = Some(result);
                    }
                }
                Ok(_) => {}
                Err(error) if first_error.is_none() => {
                    cancelled.store(true, AtomicOrdering::Release);
                    first_error = Some(error);
                }
                Err(_) => {}
            }
        }
    });

    if let Some(error) = first_error {
        return Err(error);
    }
    records.flush()?;
    outputs.flush()?;
    results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.ok_or_else(|| {
                io::Error::other(format!(
                    "parallel reverse search produced no result for input {}",
                    index + 1
                ))
            })
        })
        .collect()
}

fn run_parallel_inmemory_bfs(
    positions: &[Position],
    out_dir: &Path,
    discs: u32,
    goal_match: GoalMatch,
    node_limit: usize,
    use_lp: bool,
    use_occupancy_cache: bool,
    threads: usize,
) -> io::Result<Vec<(GoalClassification, usize)>> {
    let threads = if threads == 0 {
        std::thread::available_parallelism()?.get().min(64)
    } else {
        threads
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(io::Error::other)?;
    pool.install(|| {
        run_queries(positions, out_dir, discs, goal_match, |_, goal, table| {
            parallel_inmemory_retrospective_bfs(
                goal,
                table,
                node_limit,
                use_lp,
                use_occupancy_cache,
            )
        })
    })
}

fn run_bfs(
    cfg: &Cfg,
    positions: &[Position],
    out_dir: &Path,
    goal_match: GoalMatch,
) -> io::Result<Vec<(GoalClassification, usize)>> {
    run_external(cfg, positions, out_dir, goal_match, false)
}
fn run_parallel_bfs(
    cfg: &Cfg,
    positions: &[Position],
    out_dir: &Path,
    goal_match: GoalMatch,
) -> io::Result<Vec<(GoalClassification, usize)>> {
    run_external(cfg, positions, out_dir, goal_match, true)
}

fn run_external(
    cfg: &Cfg,
    positions: &[Position],
    out_dir: &Path,
    goal_match: GoalMatch,
    parallel: bool,
) -> io::Result<Vec<(GoalClassification, usize)>> {
    use serde_json::{json, Value};
    let candidate_count = match goal_match {
        GoalMatch::Exact => positions.iter().try_fold(0_usize, |total, &goal| {
            total
                .checked_add(crate::query::candidates(Position::initial(), goal, goal_match).len())
                .ok_or_else(|| io::Error::other("candidate count overflow"))
        })?,
        GoalMatch::EquivalenceClass => positions.len(),
    };
    let identity = json!({"format": "reverse-query-batch-v7", "mode": goal_match.as_str(),
        "positions": positions.iter().copied().map(format_position).collect::<Vec<_>>(),
        "discs": cfg.discs, "jobs": cfg.jobs,
        "block_size": cfg.block_size, "parallel": parallel});
    fs::create_dir_all(&cfg.tmp_dir)?;
    let manifest_path = cfg.tmp_dir.join("batch.json");
    let mut manifest = if cfg.resume {
        let saved: Value = serde_json::from_reader(File::open(&manifest_path)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if saved.get("identity") != Some(&identity) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "saved batch input or settings mismatch",
            ));
        }
        saved
    } else {
        if fs::read_dir(&cfg.tmp_dir)?.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "temporary directory is not empty",
            ));
        }
        json!({"identity": identity, "completed": 0, "active": null})
    };
    let completed = manifest
        .get("completed")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n <= candidate_count)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid batch progress"))?;
    let active = match manifest.get("active") {
        Some(Value::Null) => None,
        Some(number) => Some(
            number
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&n| n == completed + 1 && n <= candidate_count)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid active query")
                })?,
        ),
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "missing active query state",
            ))
        }
    };
    save_manifest(&manifest_path, &manifest)?;
    let mut candidate_index = 0;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(cfg.jobs)
        .build()
        .map_err(io::Error::other)?;
    pool.install(|| {
        run_queries(
            positions,
            out_dir,
            cfg.discs as u32,
            goal_match,
            |_, goal, table| {
                candidate_index += 1;
                let index = candidate_index;
                let resume = cfg.resume && (index <= completed || active == Some(index));
                let query_cfg = Cfg {
                    tmp_dir: cfg.tmp_dir.join(format!("candidate_{index}")),
                    resume,
                    ..cfg.clone()
                };
                if resume && index <= completed {
                    let saved_query: Value = serde_json::from_reader(File::open(
                        query_cfg.tmp_dir.join("checkpoint.json"),
                    )?)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    if !matches!(
                        saved_query.get("state").and_then(Value::as_str),
                        Some("found" | "not-found")
                    ) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "completed batch query has no completed search checkpoint",
                        ));
                    }
                }
                if index > completed {
                    if !resume {
                        validate_fresh_directory(&query_cfg.tmp_dir)?;
                    }
                    manifest["active"] = json!(index);
                    save_manifest(&manifest_path, &manifest)?;
                }
                let outcome = if parallel {
                    parallel_retrospective_bfs(&query_cfg, goal, table)?
                } else {
                    sequential_retrospective_bfs(&query_cfg, goal, table)?
                };
                if index > completed {
                    manifest["completed"] = json!(index);
                    manifest["active"] = Value::Null;
                    save_manifest(&manifest_path, &manifest)?;
                }
                Ok(outcome)
            },
        )
    })
}

fn save_manifest(path: &Path, manifest: &serde_json::Value) -> io::Result<()> {
    let temporary = path.with_extension("pending");
    let mut file = File::create(&temporary)?;
    serde_json::to_writer_pretty(&mut file, manifest).map_err(io::Error::other)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    sync_parent(path)
}

fn external_cfg(config: &ExternalBfs) -> Cfg {
    Cfg {
        jobs: config.jobs,
        verbose: config.verbose,
        block_size: config.block_size,
        discs: config.discs as usize,
        tmp_dir: config.tmp_dir.clone(),
        resume: config.resume,
    }
}

pub fn run_strategy(
    strategy: &Strategy,
    positions: &[Position],
    out_dir: &Path,
    goal_match: GoalMatch,
) -> io::Result<Vec<(GoalClassification, usize)>> {
    strategy.validate()?;
    for (index, position) in positions.iter().enumerate() {
        validate_board(&position.board).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("position {}: invalid board: {error:?}", index + 1),
            )
        })?;
    }
    match strategy {
        Strategy::Dfs { discs, max_nodes } => {
            let mut visited = Btable::new(0x1_0000_0000, 0x1_0000);
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .map_err(io::Error::other)?;
            pool.install(|| {
                run_queries(positions, out_dir, *discs, goal_match, |_, goal, table| {
                    visited.clear();
                    retrospective_search(goal, table, &mut visited, *max_nodes)
                })
            })
        }
        Strategy::DfsMoveOrdering { discs, max_nodes } => {
            let mut visited = Btable::new(0x1_0000_0000, 0x1_0000);
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .map_err(io::Error::other)?;
            pool.install(|| {
                run_queries(positions, out_dir, *discs, goal_match, |_, goal, table| {
                    visited.clear();
                    retrospective_search_move_ordering(goal, table, &mut visited, *max_nodes)
                })
            })
        }
        Strategy::DfsParallel {
            discs,
            max_nodes,
            table_size,
            threads,
        } => {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(*threads)
                .build()
                .map_err(io::Error::other)?;
            pool.install(|| {
                run_queries(positions, out_dir, *discs, goal_match, |_, goal, table| {
                    retrospective_search_parallel(goal, table, *max_nodes, *table_size)
                })
            })
        }
        Strategy::GbfsParallel {
            discs,
            max_nodes,
            use_lp,
            threads,
            parallel_goals,
        } => {
            let threads = if *threads == 0 {
                std::thread::available_parallelism()?.get().min(64)
            } else {
                *threads
            };
            run_parallel_queries(
                positions,
                out_dir,
                *discs,
                goal_match,
                *parallel_goals,
                threads,
                |_, goal, table| {
                    parallel_retrospective_greedy_best_first_search(
                        goal, table, *max_nodes, *use_lp, threads,
                    )
                },
            )
        }
        Strategy::BfsExternal(config) => {
            run_bfs(&external_cfg(config), positions, out_dir, goal_match)
        }
        Strategy::BfsExternalParallel(config) => {
            run_parallel_bfs(&external_cfg(config), positions, out_dir, goal_match)
        }
        Strategy::BfsInmemoryParallel {
            discs,
            max_nodes,
            use_lp,
            use_occupancy_cache,
            threads,
        } => run_parallel_inmemory_bfs(
            positions,
            out_dir,
            *discs,
            goal_match,
            *max_nodes,
            *use_lp,
            *use_occupancy_cache,
            *threads,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use crate::othello::Board;
    use crate::record::ReachabilityRecord;
    use crate::search::core::successors;

    static DIRECTORY_INDEX: AtomicU64 = AtomicU64::new(0);

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "othello-search-{name}-{}-{}",
                std::process::id(),
                DIRECTORY_INDEX.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn join(&self, path: impl AsRef<Path>) -> PathBuf {
            self.0.join(path)
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn external_config(discs: u32, block_size: usize, tmp_dir: PathBuf) -> ExternalBfs {
        ExternalBfs {
            discs,
            jobs: 1,
            verbose: 0,
            block_size,
            tmp_dir,
            resume: false,
        }
    }

    fn assert_no_reverse_outputs(out_dir: &Path) {
        for suffix in ["OK.txt", "NG.txt", "UNKNOWN.txt", "records.csv"] {
            assert!(!out_dir.join(format!("reverse_exact_{suffix}")).exists());
        }
    }

    fn gbfs_strategy(parallel_goals: usize, threads: usize) -> Strategy {
        Strategy::GbfsParallel {
            discs: 4,
            max_nodes: usize::MAX,
            use_lp: false,
            threads,
            parallel_goals,
        }
    }

    fn query_positions_with_duplicate() -> [Position; 4] {
        let after_move = successors(Position::initial())[0].0;
        let wrong_turn = Position {
            side_to_move: after_move.side_to_move.opposite(),
            ..after_move
        };
        [Position::initial(), after_move, wrong_turn, after_move]
    }

    fn reachable_position_with_discs(discs: u32) -> Position {
        let mut position = Position::initial();
        while position.board.popcount() < discs {
            position = successors(position)
                .into_iter()
                .next()
                .expect("the fixed opening reaches the requested stone count")
                .0;
        }
        assert_eq!(position.board.popcount(), discs);
        position
    }

    fn output_lines(out_dir: &Path, suffix: &str) -> Vec<String> {
        fs::read_to_string(out_dir.join(format!("reverse_exact_{suffix}.txt")))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn output_records(out_dir: &Path) -> Vec<ReachabilityRecord> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_path(out_dir.join("reverse_exact_records.csv"))
            .unwrap();
        reader
            .records()
            .map(|fields| ReachabilityRecord::try_from(&fields.unwrap()).unwrap())
            .collect()
    }

    #[test]
    fn invalid_position_is_rejected_before_output_files_are_created() {
        let directory = TemporaryDirectory::new("invalid-position");
        let out_dir = directory.join("out");
        fs::create_dir(&out_dir).unwrap();
        let mut positions = vec![Position::initial(); 4];
        positions[3].board = Board::empty();

        let error = run_strategy(
            &Strategy::Dfs {
                discs: 4,
                max_nodes: 1,
            },
            &positions,
            &out_dir,
            GoalMatch::Exact,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("position 4"));
        assert_no_reverse_outputs(&out_dir);
    }

    #[test]
    fn every_strategy_rejects_invalid_discs_before_creating_outputs() {
        let directory = TemporaryDirectory::new("invalid-discs");
        let strategies = [
            Strategy::Dfs {
                discs: 3,
                max_nodes: 1,
            },
            Strategy::DfsMoveOrdering {
                discs: 3,
                max_nodes: 1,
            },
            Strategy::DfsParallel {
                discs: 3,
                max_nodes: 1,
                table_size: 1,
                threads: 1,
            },
            Strategy::GbfsParallel {
                discs: 3,
                max_nodes: 1,
                use_lp: false,
                threads: 1,
                parallel_goals: 1,
            },
            Strategy::BfsExternal(external_config(3, 1, directory.join("external-tmp"))),
            Strategy::BfsExternalParallel(external_config(
                3,
                1,
                directory.join("external-parallel-tmp"),
            )),
            Strategy::BfsInmemoryParallel {
                discs: 3,
                max_nodes: 1,
                use_lp: false,
                use_occupancy_cache: false,
                threads: 1,
            },
        ];

        for (index, strategy) in strategies.into_iter().enumerate() {
            let out_dir = directory.join(format!("out-{index}"));
            fs::create_dir(&out_dir).unwrap();
            let error = run_strategy(
                &strategy,
                &[Position::initial()],
                &out_dir,
                GoalMatch::Exact,
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_no_reverse_outputs(&out_dir);
        }
        assert!(!directory.join("external-tmp/batch.json").exists());
        assert!(!directory.join("external-parallel-tmp/batch.json").exists());
    }

    #[test]
    fn external_strategies_reject_zero_block_size_before_creating_batch() {
        let directory = TemporaryDirectory::new("zero-block-size");
        for (index, strategy) in [
            Strategy::BfsExternal(external_config(4, 0, directory.join("tmp-0"))),
            Strategy::BfsExternalParallel(external_config(4, 0, directory.join("tmp-1"))),
        ]
        .into_iter()
        .enumerate()
        {
            let out_dir = directory.join(format!("out-{index}"));
            fs::create_dir(&out_dir).unwrap();
            let error = run_strategy(
                &strategy,
                &[Position::initial()],
                &out_dir,
                GoalMatch::Exact,
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_no_reverse_outputs(&out_dir);
            assert!(!directory.join(format!("tmp-{index}/batch.json")).exists());
        }
    }

    #[test]
    fn gbfs_parallel_validates_goal_and_thread_parallelism() {
        for (parallel_goals, threads, message) in [
            (0, 1, "parallel goals must be positive"),
            (
                2,
                0,
                "threads must be positive when parallel goals is at least 2",
            ),
            (3, 22, "parallel goals times threads must not exceed 64"),
        ] {
            let error = gbfs_strategy(parallel_goals, threads)
                .validate()
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), message);
        }

        for (parallel_goals, threads) in [(1, 0), (1, usize::MAX), (2, 32), (64, 1)] {
            gbfs_strategy(parallel_goals, threads).validate().unwrap();
        }
    }

    #[test]
    fn gbfs_parallel_goal_counts_match_results_files_and_records() {
        let directory = TemporaryDirectory::new("parallel-goal-counts");
        let positions = query_positions_with_duplicate();
        let serial_dir = directory.join("serial");
        let parallel_dir = directory.join("parallel");

        let serial = run_strategy(
            &gbfs_strategy(1, 1),
            &positions,
            &serial_dir,
            GoalMatch::Exact,
        )
        .unwrap();
        let parallel = run_strategy(
            &gbfs_strategy(4, 1),
            &positions,
            &parallel_dir,
            GoalMatch::Exact,
        )
        .unwrap();

        assert_eq!(parallel, serial);
        for suffix in ["OK", "NG", "UNKNOWN"] {
            let mut serial_lines = output_lines(&serial_dir, suffix);
            let mut parallel_lines = output_lines(&parallel_dir, suffix);
            serial_lines.sort_unstable();
            parallel_lines.sort_unstable();
            assert_eq!(parallel_lines, serial_lines, "{suffix}");
        }
        let mut serial_records = output_records(&serial_dir);
        let mut parallel_records = output_records(&parallel_dir);
        serial_records.sort_by_key(|record| record.input_index);
        parallel_records.sort_by_key(|record| record.input_index);
        assert_eq!(parallel_records, serial_records);
        assert_eq!(
            parallel_records
                .iter()
                .map(|record| record.input_index)
                .collect::<Vec<_>>(),
            [1, 2, 4]
        );
        for record in parallel_records {
            record.verify().unwrap();
        }
    }

    #[test]
    fn one_parallel_goal_preserves_input_order_and_duplicates() {
        let directory = TemporaryDirectory::new("one-parallel-goal");
        let positions = query_positions_with_duplicate();
        let results = run_strategy(
            &gbfs_strategy(1, 1),
            &positions,
            &directory.0,
            GoalMatch::Exact,
        )
        .unwrap();

        assert_eq!(results.len(), positions.len());
        for (index, ((classification, _), position)) in results.iter().zip(positions).enumerate() {
            assert_eq!(classification.input_index(), index + 1);
            assert_eq!(classification.goal(), position);
        }
        let after_move = format_position(positions[1]);
        assert_eq!(
            output_lines(&directory.0, "OK"),
            [
                format_position(positions[0]),
                after_move.clone(),
                after_move
            ]
        );
        assert_eq!(
            output_lines(&directory.0, "NG"),
            [format_position(positions[2])]
        );
        assert!(output_lines(&directory.0, "UNKNOWN").is_empty());
    }

    #[test]
    fn parallel_query_error_cancels_unstarted_positions() {
        let directory = TemporaryDirectory::new("parallel-query-error");
        let positions = vec![Position::initial(); 32];
        let started = AtomicUsize::new(0);

        let error = run_parallel_queries(
            &positions,
            &directory.0,
            4,
            GoalMatch::Exact,
            4,
            1,
            |input_index, _, _| {
                started.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::other(format!("failure at input {input_index}")))
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("failure at input"));
        assert!((1..=4).contains(&started.load(Ordering::SeqCst)));
        for suffix in ["OK", "NG", "UNKNOWN"] {
            assert!(output_lines(&directory.0, suffix).is_empty());
        }
        assert!(output_records(&directory.0).is_empty());
    }

    #[test]
    fn empty_parallel_query_batch_creates_empty_outputs() {
        let directory = TemporaryDirectory::new("empty-parallel-query-batch");
        let results = run_parallel_queries(
            &[],
            &directory.0,
            4,
            GoalMatch::Exact,
            4,
            1,
            |_, _, _| -> io::Result<SearchOutcome> {
                panic!("an empty batch must not invoke solve")
            },
        )
        .unwrap();

        assert!(results.is_empty());
        for suffix in ["OK", "NG", "UNKNOWN"] {
            assert!(output_lines(&directory.0, suffix).is_empty());
        }
        assert!(output_records(&directory.0).is_empty());
    }

    #[test]
    fn candidate_table_rejects_a_different_goal() {
        let goals = successors(Position::initial());
        let goal = goals[0].0;
        let different_goal = goals[1].0;

        let (classification, nodes) =
            classify_position(1, goal, 4, GoalMatch::Exact, None, |_, candidate, table| {
                table.validate_goal(candidate)?;
                let error = table.validate_goal(different_goal).unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
                Ok(SearchOutcome::Unknown { nodes: 7 })
            })
            .unwrap();

        assert_eq!(classification.goal(), goal);
        assert_eq!(classification.category(), TriCategory::Unknown);
        assert_eq!(nodes, 7);
    }

    #[test]
    fn equivalence_classification_searches_the_input_once() {
        let goal = successors(Position::initial())[0].0;
        let calls = AtomicUsize::new(0);

        let (classification, nodes) = classify_position(
            3,
            goal,
            4,
            GoalMatch::EquivalenceClass,
            None,
            |input_index, candidate, table| {
                calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(input_index, 3);
                assert_eq!(candidate, goal);
                assert_eq!(table.goal_match(), GoalMatch::EquivalenceClass);
                table.validate_goal(goal)?;
                Ok(SearchOutcome::NotFound { nodes: 11 })
            },
        )
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(nodes, 11);
        assert_eq!(classification.input_index(), 3);
        assert_eq!(classification.goal(), goal);
        assert_eq!(classification.goal_match(), GoalMatch::EquivalenceClass);
        assert_eq!(classification.category(), TriCategory::Ng);
    }

    #[test]
    fn table_pool_covers_gbfs_with_scoped_workers() {
        let goal = reachable_position_with_discs(7);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        let pool_threads = AtomicUsize::new(0);

        let (classification, nodes) = classify_position(
            1,
            goal,
            4,
            GoalMatch::Exact,
            Some(&pool),
            |_, candidate, table| {
                pool_threads.store(rayon::current_num_threads(), Ordering::SeqCst);
                parallel_retrospective_greedy_best_first_search(
                    candidate,
                    table,
                    usize::MAX,
                    false,
                    2,
                )
            },
        )
        .unwrap();

        assert_eq!(pool_threads.load(Ordering::SeqCst), 2);
        assert!(nodes > 0);
        classification.record().unwrap().verify().unwrap();
    }

    #[test]
    fn external_equivalence_uses_one_checkpoint_directory_per_input() {
        let directory = TemporaryDirectory::new("external-equivalence-inputs");
        let tmp_dir = directory.join("tmp");
        let positions = [Position::initial(), successors(Position::initial())[0].0];
        let results = run_strategy(
            &Strategy::BfsExternal(external_config(4, 8, tmp_dir.clone())),
            &positions,
            &directory.join("out"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        assert_eq!(results.len(), positions.len());
        for (index, ((classification, _), goal)) in results.iter().zip(positions).enumerate() {
            let record = classification.record().unwrap();
            assert_eq!(record.input_index, index + 1);
            assert_eq!(record.goal, goal);
            assert_eq!(record.goal_match, GoalMatch::EquivalenceClass);
            record.verify().unwrap();
        }
        assert!(tmp_dir.join("candidate_1/checkpoint.json").is_file());
        assert!(tmp_dir.join("candidate_2/checkpoint.json").is_file());
        assert!(!tmp_dir.join("candidate_3").exists());
        let checkpoint: serde_json::Value = serde_json::from_reader(
            File::open(tmp_dir.join("candidate_1/checkpoint.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            checkpoint["identity"]["goal_match"],
            GoalMatch::EquivalenceClass.as_str()
        );
        let manifest: serde_json::Value =
            serde_json::from_reader(File::open(tmp_dir.join("batch.json")).unwrap()).unwrap();
        assert_eq!(manifest["identity"]["format"], "reverse-query-batch-v7");
        assert_eq!(manifest["completed"], positions.len());
    }

    #[test]
    fn completed_external_equivalence_search_resumes_with_the_same_record() {
        let directory = TemporaryDirectory::new("external-equivalence-completed-resume");
        let tmp_dir = directory.join("tmp");
        let goal = reachable_position_with_discs(7);
        let config = external_config(4, 4, tmp_dir);
        let first = run_strategy(
            &Strategy::BfsExternal(config.clone()),
            &[goal],
            &directory.join("first"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        let resumed = run_strategy(
            &Strategy::BfsExternal(ExternalBfs {
                resume: true,
                ..config
            }),
            &[goal],
            &directory.join("resumed"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        assert_eq!(resumed, first);
        resumed[0].0.record().unwrap().verify().unwrap();
    }

    #[test]
    fn active_batch_without_a_query_checkpoint_restarts_that_query() {
        let directory = TemporaryDirectory::new("external-before-first-checkpoint");
        let tmp_dir = directory.join("tmp");
        let candidate_dir = tmp_dir.join("candidate_1");
        fs::create_dir_all(&candidate_dir).unwrap();
        fs::write(candidate_dir.join("layer_0.bin"), b"incomplete layer").unwrap();
        fs::write(candidate_dir.join("layer_0.pending"), b"incomplete layer").unwrap();
        fs::write(
            candidate_dir.join("checkpoint.pending"),
            b"incomplete checkpoint",
        )
        .unwrap();
        let goal = reachable_position_with_discs(7);
        let config = external_config(4, 4, tmp_dir.clone());
        let identity = serde_json::json!({
            "format": "reverse-query-batch-v7",
            "mode": GoalMatch::EquivalenceClass.as_str(),
            "positions": [format_position(goal)],
            "discs": config.discs,
            "jobs": config.jobs,
            "block_size": config.block_size,
            "parallel": false,
        });
        save_manifest(
            &tmp_dir.join("batch.json"),
            &serde_json::json!({"identity": identity, "completed": 0, "active": 1}),
        )
        .unwrap();

        let results = run_strategy(
            &Strategy::BfsExternal(ExternalBfs {
                resume: true,
                ..config
            }),
            &[goal],
            &directory.join("out"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        results[0].0.record().unwrap().verify().unwrap();
        assert!(candidate_dir.join("checkpoint.json").is_file());
        assert!(!candidate_dir.join("layer_0.pending").exists());
        assert!(!candidate_dir.join("checkpoint.pending").exists());
        let manifest: serde_json::Value =
            serde_json::from_reader(File::open(tmp_dir.join("batch.json")).unwrap()).unwrap();
        assert_eq!(manifest["completed"], 1);
        assert!(manifest["active"].is_null());
    }

    #[test]
    fn unstarted_external_candidate_preserves_user_files_on_repeated_resume() {
        let directory = TemporaryDirectory::new("external-unstarted-user-file");
        let tmp_dir = directory.join("tmp");
        let positions = [Position::initial(), reachable_position_with_discs(7)];
        let config = external_config(4, 4, tmp_dir.clone());
        run_strategy(
            &Strategy::BfsExternal(config.clone()),
            &positions,
            &directory.join("initial"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        let candidate_dir = tmp_dir.join("candidate_2");
        fs::remove_dir_all(&candidate_dir).unwrap();
        fs::create_dir(&candidate_dir).unwrap();
        let user_file = candidate_dir.join("notes.txt");
        fs::write(&user_file, b"user data").unwrap();
        let manifest_path = tmp_dir.join("batch.json");
        let mut manifest: serde_json::Value =
            serde_json::from_reader(File::open(&manifest_path).unwrap()).unwrap();
        manifest["completed"] = serde_json::json!(1);
        manifest["active"] = serde_json::Value::Null;
        save_manifest(&manifest_path, &manifest).unwrap();

        let resume = Strategy::BfsExternal(ExternalBfs {
            resume: true,
            ..config
        });
        for attempt in 1..=2 {
            let error = run_strategy(
                &resume,
                &positions,
                &directory.join(format!("resume-{attempt}")),
                GoalMatch::EquivalenceClass,
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(&user_file).unwrap(), b"user data");
        }
        let manifest: serde_json::Value =
            serde_json::from_reader(File::open(manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["completed"], 1);
        assert!(manifest["active"].is_null());
    }

    #[test]
    fn active_external_equivalence_search_resumes_to_completion() {
        let directory = TemporaryDirectory::new("external-equivalence-active-resume");
        let tmp_dir = directory.join("tmp");
        let goal = reachable_position_with_discs(7);
        let config = external_config(4, 4, tmp_dir.clone());
        let first = run_strategy(
            &Strategy::BfsExternal(config.clone()),
            &[goal],
            &directory.join("first"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        let checkpoint_path = tmp_dir.join("candidate_1/checkpoint.json");
        let mut checkpoint: serde_json::Value =
            serde_json::from_reader(File::open(&checkpoint_path).unwrap()).unwrap();
        assert_eq!(checkpoint["state"], "found");
        checkpoint["state"] = serde_json::json!("searching");
        checkpoint["record"] = serde_json::Value::Null;
        save_manifest(&checkpoint_path, &checkpoint).unwrap();

        let manifest_path = tmp_dir.join("batch.json");
        let mut manifest: serde_json::Value =
            serde_json::from_reader(File::open(&manifest_path).unwrap()).unwrap();
        manifest["completed"] = serde_json::json!(0);
        manifest["active"] = serde_json::json!(1);
        save_manifest(&manifest_path, &manifest).unwrap();

        let resumed = run_strategy(
            &Strategy::BfsExternal(ExternalBfs {
                resume: true,
                ..config
            }),
            &[goal],
            &directory.join("resumed"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        assert_eq!(resumed[0].0, first[0].0);
        resumed[0].0.record().unwrap().verify().unwrap();
        let manifest: serde_json::Value =
            serde_json::from_reader(File::open(manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["completed"], 1);
        assert!(manifest["active"].is_null());
    }

    #[test]
    fn external_resume_rejects_a_different_goal_match_identity() {
        let directory = TemporaryDirectory::new("external-goal-match-resume");
        let tmp_dir = directory.join("tmp");
        let goal = reachable_position_with_discs(7);
        let config = external_config(4, 4, tmp_dir.clone());
        run_strategy(
            &Strategy::BfsExternal(config.clone()),
            &[goal],
            &directory.join("exact"),
            GoalMatch::Exact,
        )
        .unwrap();
        let resume = Strategy::BfsExternal(ExternalBfs {
            resume: true,
            ..config
        });

        let error = run_strategy(
            &resume,
            &[goal],
            &directory.join("equivalence"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("saved batch input or settings mismatch"));

        let manifest_path = tmp_dir.join("batch.json");
        let mut manifest: serde_json::Value =
            serde_json::from_reader(File::open(&manifest_path).unwrap()).unwrap();
        manifest["identity"]["mode"] = serde_json::json!("equivalence");
        save_manifest(&manifest_path, &manifest).unwrap();
        let error = run_strategy(
            &resume,
            &[goal],
            &directory.join("equivalence-inner"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("checkpoint query or search settings mismatch"));
    }

    #[test]
    fn external_search_rejects_a_table_with_another_boundary() {
        let directory = TemporaryDirectory::new("external-table-boundary");
        let goal = successors(Position::initial())[0].0;
        let table = ForwardTable::for_equivalence(4, goal).unwrap();
        let tmp_dir = directory.join("tmp");
        let cfg = Cfg {
            jobs: 1,
            verbose: 0,
            block_size: 8,
            discs: 5,
            tmp_dir: tmp_dir.clone(),
            resume: false,
        };

        let error = sequential_retrospective_bfs(&cfg, goal, &table).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!tmp_dir.exists());
    }

    #[test]
    fn mixed_five_and_ten_disc_positions_are_classified_independently() {
        let directory = TemporaryDirectory::new("mixed-stone-counts");
        let positions = [
            reachable_position_with_discs(5),
            reachable_position_with_discs(10),
        ];

        let results = run_strategy(
            &Strategy::Dfs {
                discs: 10,
                max_nodes: 1,
            },
            &positions,
            &directory.0,
            GoalMatch::Exact,
        )
        .unwrap();

        assert_eq!(results.len(), positions.len());
        for (index, ((classification, _), expected)) in results.iter().zip(positions).enumerate() {
            assert_eq!(classification.input_index(), index + 1);
            assert_eq!(classification.goal(), expected);
            classification
                .record()
                .expect("each fixed opening position is reachable")
                .verify()
                .unwrap();
        }
        assert_eq!(
            output_lines(&directory.0, "OK"),
            positions.map(format_position)
        );
        assert!(output_lines(&directory.0, "NG").is_empty());
        assert!(output_lines(&directory.0, "UNKNOWN").is_empty());
    }

    #[test]
    fn results_preserve_input_order_and_duplicate_occurrences() {
        let directory = TemporaryDirectory::new("ordered-results");
        let out_dir = directory.join("out");
        let after_move = successors(Position::initial())[0].0;
        let positions = [Position::initial(), after_move, Position::initial()];

        let results = run_strategy(
            &Strategy::Dfs {
                discs: 5,
                max_nodes: 1,
            },
            &positions,
            &out_dir,
            GoalMatch::Exact,
        )
        .unwrap();

        assert_eq!(results.len(), positions.len());
        for (index, ((classification, _), expected)) in results.iter().zip(positions).enumerate() {
            assert_eq!(classification.input_index(), index + 1);
            assert_eq!(classification.goal(), expected);
        }
        assert_eq!(results[0].0.goal(), results[2].0.goal());
        assert_ne!(results[0].0.input_index(), results[2].0.input_index());
    }
}
