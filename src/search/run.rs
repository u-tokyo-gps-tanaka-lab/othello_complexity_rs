use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::io::{ensure_tri_outputs, format_position};
use crate::othello::{validate_board, Position};
use crate::query::{solve_query, ExactOutcome, GoalClassification};
use crate::record::GoalMatch;
use crate::search::{
    core::SearchOutcome,
    dfs::retrospective_search,
    external_bfs::{parallel_retrospective_bfs, sequential_retrospective_bfs, Cfg},
    forward::ForwardTable,
    inmemory_bfs::parallel_inmemory_retrospective_bfs,
    move_ordering::retrospective_search_move_ordering,
    parallel_dfs::retrospective_search_parallel,
    parallel_gbfs::parallel_retrospective_greedy_best_first_search,
    transposition::Btable,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForwardMode {
    Shared,
    Targeted,
}

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
        Ok(())
    }
}

fn forward_mode(strategy: &Strategy) -> ForwardMode {
    match strategy {
        Strategy::Dfs { .. }
        | Strategy::DfsMoveOrdering { .. }
        | Strategy::DfsParallel { .. }
        | Strategy::GbfsParallel { .. } => ForwardMode::Shared,
        Strategy::BfsExternal(_)
        | Strategy::BfsExternalParallel(_)
        | Strategy::BfsInmemoryParallel { .. } => ForwardMode::Targeted,
    }
}

fn run_queries(
    positions: &[Position],
    out_dir: &Path,
    discs: u32,
    goal_match: GoalMatch,
    forward_mode: ForwardMode,
    mut solve: impl FnMut(usize, Position, &ForwardTable) -> io::Result<SearchOutcome>,
) -> io::Result<Vec<(GoalClassification, usize)>> {
    let prefix = format!("reverse_{goal_match}");
    let mut outputs = ensure_tri_outputs(out_dir, &prefix)?;
    let mut records = BufWriter::new(File::create(out_dir.join(format!("{prefix}_records.csv")))?);
    let mut results = Vec::with_capacity(positions.len());
    // DFS and GBFS share an unpruned table. No input needs states above the
    // largest target, and an empty input needs no forward search at all.
    let shared_table = match forward_mode {
        ForwardMode::Shared => positions
            .iter()
            .map(|position| position.board.popcount())
            .max()
            .map(|maximum| ForwardTable::new(discs.min(maximum), None))
            .transpose()?,
        ForwardMode::Targeted => None,
    };
    for (index, position) in positions.iter().copied().enumerate() {
        let mut nodes = 0_usize;
        let outcome = solve_query(
            Position::initial(),
            position,
            index + 1,
            goal_match,
            |candidate, _transform| {
                let targeted_table;
                let table = match &shared_table {
                    Some(table) => table,
                    None => {
                        targeted_table = ForwardTable::new(discs, Some(candidate))
                            .map_err(|error| error.to_string())?;
                        &targeted_table
                    }
                };
                let outcome =
                    solve(index + 1, candidate, table).map_err(|error| error.to_string())?;
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
        if let Some(record) = outcome.record() {
            writeln!(records, "{}", record.to_csv())?;
            records.flush()?;
        }
        let line = format_position(position);
        outputs.write_result(&outcome, &line)?;
        outputs.flush()?;
        let result = match crate::io::TriOutcome::category(&outcome) {
            crate::io::TriCategory::Ok => "OK",
            crate::io::TriCategory::Ng => "NG",
            crate::io::TriCategory::Unknown => "UNKNOWN",
        };
        println!("input {}: {result} nodes={nodes}", index + 1);
        results.push((outcome, nodes));
    }
    records.flush()?;
    outputs.flush()?;
    Ok(results)
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
        run_queries(
            positions,
            out_dir,
            discs,
            goal_match,
            ForwardMode::Targeted,
            |_, goal, table| {
                parallel_inmemory_retrospective_bfs(
                    goal,
                    table,
                    node_limit,
                    use_lp,
                    use_occupancy_cache,
                )
            },
        )
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
    let candidate_count = positions.iter().try_fold(0_usize, |total, &goal| {
        total
            .checked_add(crate::query::candidates(Position::initial(), goal, goal_match).len())
            .ok_or_else(|| io::Error::other("candidate count overflow"))
    })?;
    let identity = json!({"format": "reverse-query-batch-v5", "mode": goal_match.as_str(),
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
    run_queries(
        positions,
        out_dir,
        cfg.discs as u32,
        goal_match,
        ForwardMode::Targeted,
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
                let saved_query: Value =
                    serde_json::from_reader(File::open(query_cfg.tmp_dir.join("checkpoint.json"))?)
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
}

fn save_manifest(path: &Path, manifest: &serde_json::Value) -> io::Result<()> {
    let temporary = path.with_extension("pending");
    let mut file = File::create(&temporary)?;
    serde_json::to_writer_pretty(&mut file, manifest).map_err(io::Error::other)?;
    file.sync_all()?;
    fs::rename(temporary, path)
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
            run_queries(
                positions,
                out_dir,
                *discs,
                goal_match,
                forward_mode(strategy),
                |_, goal, table| {
                    visited.clear();
                    retrospective_search(goal, table, &mut visited, *max_nodes)
                },
            )
        }
        Strategy::DfsMoveOrdering { discs, max_nodes } => {
            let mut visited = Btable::new(0x1_0000_0000, 0x1_0000);
            run_queries(
                positions,
                out_dir,
                *discs,
                goal_match,
                forward_mode(strategy),
                |_, goal, table| {
                    visited.clear();
                    retrospective_search_move_ordering(goal, table, &mut visited, *max_nodes)
                },
            )
        }
        Strategy::DfsParallel {
            discs,
            max_nodes,
            table_size,
            threads,
        } => run_queries(
            positions,
            out_dir,
            *discs,
            goal_match,
            forward_mode(strategy),
            |_, goal, table| {
                retrospective_search_parallel(goal, table, *max_nodes, *table_size, *threads)
            },
        ),
        Strategy::GbfsParallel {
            discs,
            max_nodes,
            use_lp,
            threads,
        } => {
            let threads = if *threads == 0 {
                std::thread::available_parallelism()?.get().min(64)
            } else {
                *threads
            };
            run_queries(
                positions,
                out_dir,
                *discs,
                goal_match,
                forward_mode(strategy),
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
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::othello::Board;
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

    #[test]
    fn strategies_select_the_expected_forward_table_mode() {
        let external = ExternalBfs {
            discs: 4,
            jobs: 1,
            verbose: 0,
            block_size: 1,
            tmp_dir: PathBuf::from("unused"),
            resume: false,
        };
        let cases = [
            (
                Strategy::Dfs {
                    discs: 4,
                    max_nodes: 1,
                },
                ForwardMode::Shared,
            ),
            (
                Strategy::DfsMoveOrdering {
                    discs: 4,
                    max_nodes: 1,
                },
                ForwardMode::Shared,
            ),
            (
                Strategy::DfsParallel {
                    discs: 4,
                    max_nodes: 1,
                    table_size: 1,
                    threads: 1,
                },
                ForwardMode::Shared,
            ),
            (
                Strategy::GbfsParallel {
                    discs: 4,
                    max_nodes: 1,
                    use_lp: false,
                    threads: 1,
                },
                ForwardMode::Shared,
            ),
            (
                Strategy::BfsExternal(external.clone()),
                ForwardMode::Targeted,
            ),
            (
                Strategy::BfsExternalParallel(external),
                ForwardMode::Targeted,
            ),
            (
                Strategy::BfsInmemoryParallel {
                    discs: 4,
                    max_nodes: 1,
                    use_lp: false,
                    use_occupancy_cache: false,
                    threads: 1,
                },
                ForwardMode::Targeted,
            ),
        ];
        for (strategy, expected) in cases {
            assert_eq!(forward_mode(&strategy), expected, "{strategy:?}");
        }
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
