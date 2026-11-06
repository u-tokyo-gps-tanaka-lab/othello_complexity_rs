//! Reverse-search strategies and dispatch.

mod batch;
pub(crate) mod constraints;
pub mod engines;
pub mod forward;
pub mod moves;
pub mod outcome;
#[cfg(test)]
mod test_support;
pub mod transposition;

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::othello::{validate_board, Position};
use crate::query::GoalClassification;
use crate::record::GoalMatch;
use crate::search::{
    batch::{run_parallel_queries, run_queries},
    engines::{
        bfs::retrospective_bfs,
        dfs::{retrospective_search, retrospective_search_move_ordering},
        external_bfs::{batch::run_external, ExternalBfs},
        parallel_dfs::retrospective_search_parallel,
        parallel_gbfs::parallel_retrospective_greedy_best_first_search,
    },
    transposition::Btable,
};

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
    Bfs {
        discs: u32,
        max_nodes: usize,
        use_lp: bool,
        threads: usize,
        parallel_goals: usize,
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
            Self::Bfs { .. } => "bfs",
        }
    }

    pub fn validate(&self) -> io::Result<()> {
        let discs = match self {
            Self::Dfs { discs, .. }
            | Self::DfsMoveOrdering { discs, .. }
            | Self::DfsParallel { discs, .. }
            | Self::GbfsParallel { discs, .. }
            | Self::Bfs { discs, .. } => *discs,
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
        let goal_parallelism = match self {
            Self::GbfsParallel {
                threads,
                parallel_goals,
                ..
            }
            | Self::Bfs {
                threads,
                parallel_goals,
                ..
            } => Some((*threads, *parallel_goals)),
            _ => None,
        };
        if let Some((threads, parallel_goals)) = goal_parallelism {
            if parallel_goals == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "parallel goals must be positive",
                ));
            }
            if parallel_goals >= 2 && threads == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "threads must be positive when parallel goals is at least 2",
                ));
            }
            if parallel_goals >= 2 && threads > 64 / parallel_goals {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "parallel goals times threads must not exceed 64",
                ));
            }
        }
        Ok(())
    }
}

fn resolve_threads(threads: usize) -> io::Result<usize> {
    if threads == 0 {
        Ok(std::thread::available_parallelism()?.get().min(64))
    } else {
        Ok(threads)
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
            let threads = resolve_threads(*threads)?;
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
            run_external(config, positions, out_dir, goal_match, false)
        }
        Strategy::BfsExternalParallel(config) => {
            run_external(config, positions, out_dir, goal_match, true)
        }
        Strategy::Bfs {
            discs,
            max_nodes,
            use_lp,
            threads,
            parallel_goals,
        } => {
            let threads = resolve_threads(*threads)?;
            run_parallel_queries(
                positions,
                out_dir,
                *discs,
                goal_match,
                *parallel_goals,
                threads,
                |_, goal, table| retrospective_bfs(goal, table, *max_nodes, *use_lp),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    use crate::io::parse_position;
    use crate::othello::Board;
    use crate::search::test_support::{
        bfs_strategy, external_config, gbfs_strategy, TemporaryDirectory,
    };

    fn assert_no_reverse_outputs(out_dir: &Path) {
        for suffix in ["OK.txt", "NG.txt", "UNKNOWN.txt", "records.csv"] {
            assert!(!out_dir.join(format!("reverse_exact_{suffix}")).exists());
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
    fn invalid_discs_and_zero_block_sizes_create_no_outputs() {
        struct Case {
            name: &'static str,
            strategy: Strategy,
            batch_path: Option<PathBuf>,
        }

        let directory = TemporaryDirectory::new("invalid-search-settings");
        let external_tmp = directory.join("external-tmp");
        let external_parallel_tmp = directory.join("external-parallel-tmp");
        let zero_tmp = directory.join("zero-tmp");
        let zero_parallel_tmp = directory.join("zero-parallel-tmp");
        let cases = [
            Case {
                name: "dfs-discs",
                strategy: Strategy::Dfs {
                    discs: 3,
                    max_nodes: 1,
                },
                batch_path: None,
            },
            Case {
                name: "ordered-dfs-discs",
                strategy: Strategy::DfsMoveOrdering {
                    discs: 3,
                    max_nodes: 1,
                },
                batch_path: None,
            },
            Case {
                name: "parallel-dfs-discs",
                strategy: Strategy::DfsParallel {
                    discs: 3,
                    max_nodes: 1,
                    table_size: 1,
                    threads: 1,
                },
                batch_path: None,
            },
            Case {
                name: "gbfs-discs",
                strategy: Strategy::GbfsParallel {
                    discs: 3,
                    max_nodes: 1,
                    use_lp: false,
                    threads: 1,
                    parallel_goals: 1,
                },
                batch_path: None,
            },
            Case {
                name: "external-discs",
                strategy: Strategy::BfsExternal(external_config(3, 1, external_tmp.clone())),
                batch_path: Some(external_tmp.join("batch.json")),
            },
            Case {
                name: "parallel-external-discs",
                strategy: Strategy::BfsExternalParallel(external_config(
                    3,
                    1,
                    external_parallel_tmp.clone(),
                )),
                batch_path: Some(external_parallel_tmp.join("batch.json")),
            },
            Case {
                name: "bfs-discs",
                strategy: Strategy::Bfs {
                    discs: 3,
                    max_nodes: 1,
                    use_lp: false,
                    threads: 1,
                    parallel_goals: 1,
                },
                batch_path: None,
            },
            Case {
                name: "external-zero-block",
                strategy: Strategy::BfsExternal(external_config(4, 0, zero_tmp.clone())),
                batch_path: Some(zero_tmp.join("batch.json")),
            },
            Case {
                name: "parallel-external-zero-block",
                strategy: Strategy::BfsExternalParallel(external_config(
                    4,
                    0,
                    zero_parallel_tmp.clone(),
                )),
                batch_path: Some(zero_parallel_tmp.join("batch.json")),
            },
        ];

        for case in cases {
            let out_dir = directory.join(format!("out-{}", case.name));
            fs::create_dir(&out_dir).unwrap();
            let error = run_strategy(
                &case.strategy,
                &[Position::initial()],
                &out_dir,
                GoalMatch::Exact,
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{}", case.name);
            assert_no_reverse_outputs(&out_dir);
            if let Some(batch_path) = case.batch_path {
                assert!(!batch_path.exists(), "{}", case.name);
            }
        }
    }

    #[test]
    fn bfs_and_gbfs_validate_goal_and_thread_parallelism() {
        for (parallel_goals, threads, message) in [
            (0, 1, "parallel goals must be positive"),
            (
                2,
                0,
                "threads must be positive when parallel goals is at least 2",
            ),
            (3, 22, "parallel goals times threads must not exceed 64"),
        ] {
            for strategy in [
                gbfs_strategy(parallel_goals, threads),
                bfs_strategy(parallel_goals, threads),
            ] {
                let error = strategy.validate().unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
                assert_eq!(error.to_string(), message);
            }
        }

        for (parallel_goals, threads) in [(1, 0), (1, usize::MAX), (2, 32), (64, 1)] {
            for strategy in [
                gbfs_strategy(parallel_goals, threads),
                bfs_strategy(parallel_goals, threads),
            ] {
                strategy.validate().unwrap();
            }
        }
    }

    #[test]
    fn bfs_strategy_passes_use_lp_to_the_search() {
        let goal =
            parse_position("--OO---XOX-O-XX--X-OOOXOXOXXO--OXXXXXXOOX-XOXXO--OOXOXXXOOOX---- b")
                .unwrap();
        let run = |use_lp| {
            let directory =
                TemporaryDirectory::new(if use_lp { "bfs-lp" } else { "bfs-without-lp" });
            run_strategy(
                &Strategy::Bfs {
                    discs: 4,
                    max_nodes: 1,
                    use_lp,
                    threads: 1,
                    parallel_goals: 1,
                },
                &[goal],
                &directory.0,
                GoalMatch::Exact,
            )
            .unwrap()
            .pop()
            .unwrap()
        };

        let (classification, nodes) = run(true);
        assert!(matches!(
            classification,
            GoalClassification::Unreachable { .. }
        ));
        assert_eq!(nodes, 0);

        let (classification, nodes) = run(false);
        assert!(matches!(classification, GoalClassification::Unknown { .. }));
        assert!(nodes >= 1);
    }
}
