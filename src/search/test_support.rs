//! Shared utilities for reverse-search unit tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::othello::Position;
use crate::search::{engines::external_bfs::ExternalBfs, moves::successors, Strategy};

static DIRECTORY_INDEX: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TemporaryDirectory(pub(crate) PathBuf);

impl TemporaryDirectory {
    pub(crate) fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "othello-search-{name}-{}-{}",
            std::process::id(),
            DIRECTORY_INDEX.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub(crate) fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub(crate) fn external_config(discs: u32, block_size: usize, tmp_dir: PathBuf) -> ExternalBfs {
    ExternalBfs {
        discs,
        jobs: 1,
        verbose: 0,
        block_size,
        tmp_dir,
        resume: false,
    }
}

pub(crate) fn gbfs_strategy(parallel_goals: usize, threads: usize) -> Strategy {
    Strategy::GbfsParallel {
        discs: 4,
        max_nodes: usize::MAX,
        use_lp: false,
        threads,
        parallel_goals,
    }
}

pub(crate) fn bfs_strategy(parallel_goals: usize, threads: usize) -> Strategy {
    Strategy::Bfs {
        discs: 4,
        max_nodes: usize::MAX,
        use_lp: false,
        threads,
        parallel_goals,
    }
}

pub(crate) fn reachable_position_with_discs(discs: u32) -> Position {
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
