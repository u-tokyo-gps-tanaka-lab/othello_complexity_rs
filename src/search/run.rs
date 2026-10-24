use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::io::{ensure_tri_outputs, format_position, parse_file_to_positions};
use crate::othello::Position;
use crate::query::{solve_query, ExactOutcome};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalBfs {
    pub discs: u32,
    pub jobs: usize,
    pub verbose: u8,
    pub block_size: usize,
    pub tmp_dir: PathBuf,
    pub resume: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    input: &Path,
    out_dir: &Path,
    discs: u32,
    goal_match: GoalMatch,
    forward_mode: ForwardMode,
    mut solve: impl FnMut(usize, Position, &ForwardTable) -> io::Result<SearchOutcome>,
) -> io::Result<Vec<usize>> {
    if !(4..=64).contains(&discs) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "discs must be between 4 and 64",
        ));
    }
    // Validate the entire input before creating classification outputs.
    let positions = parse_file_to_positions(&input.to_string_lossy())?;
    let prefix = format!("reverse_{goal_match}");
    let mut outputs = ensure_tri_outputs(out_dir, &prefix)?;
    let mut records = BufWriter::new(File::create(out_dir.join(format!("{prefix}_records.csv")))?);
    let mut node_counts = Vec::with_capacity(positions.len());
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
    for (index, position) in positions.into_iter().enumerate() {
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
        node_counts.push(nodes);
    }
    records.flush()?;
    outputs.flush()?;
    Ok(node_counts)
}

pub fn run_parallel_inmemory_bfs(
    input: &Path,
    out_dir: &Path,
    discs: u32,
    goal_match: GoalMatch,
    node_limit: usize,
    use_lp: bool,
    use_occupancy_cache: bool,
    threads: usize,
) -> io::Result<Vec<usize>> {
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
            input,
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

pub fn run_bfs(cfg: &Cfg, goal_match: GoalMatch) -> io::Result<Vec<usize>> {
    run_external(cfg, goal_match, false)
}
pub fn run_parallel_bfs(cfg: &Cfg, goal_match: GoalMatch) -> io::Result<Vec<usize>> {
    run_external(cfg, goal_match, true)
}

fn run_external(cfg: &Cfg, goal_match: GoalMatch, parallel: bool) -> io::Result<Vec<usize>> {
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    let positions = parse_file_to_positions(&cfg.input.to_string_lossy())?;
    let candidate_count = positions.iter().try_fold(0_usize, |total, &goal| {
        total
            .checked_add(crate::query::candidates(Position::initial(), goal, goal_match).len())
            .ok_or_else(|| io::Error::other("candidate count overflow"))
    })?;
    let identity = json!({"format": "reverse-query-batch-v4", "mode": goal_match.as_str(),
        "input_sha256": format!("{:x}", Sha256::digest(fs::read(&cfg.input)?)),
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
        &cfg.input,
        &cfg.out_dir,
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

fn external_cfg(config: &ExternalBfs, input: &Path, out_dir: &Path) -> Cfg {
    Cfg {
        input: input.to_path_buf(),
        out_dir: out_dir.to_path_buf(),
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
    input: &Path,
    out_dir: &Path,
    goal_match: GoalMatch,
) -> io::Result<Vec<usize>> {
    match strategy {
        Strategy::Dfs { discs, max_nodes } => {
            let mut visited = Btable::new(0x1_0000_0000, 0x1_0000);
            run_queries(
                input,
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
                input,
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
            input,
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
                input,
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
        Strategy::BfsExternal(config) => run_bfs(&external_cfg(config, input, out_dir), goal_match),
        Strategy::BfsExternalParallel(config) => {
            run_parallel_bfs(&external_cfg(config, input, out_dir), goal_match)
        }
        Strategy::BfsInmemoryParallel {
            discs,
            max_nodes,
            use_lp,
            use_occupancy_cache,
            threads,
        } => run_parallel_inmemory_bfs(
            input,
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
}
