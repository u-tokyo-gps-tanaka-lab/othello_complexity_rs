//! Disk-backed stone-layer BFS with portable, query-bound checkpoints.

pub(in crate::search) mod batch;
mod expand;
mod io;
mod merge;
mod search;

pub use search::{parallel_retrospective_bfs, sequential_retrospective_bfs};

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// External BFS persists exact queries and the settings below. Resume uses the
/// same temporary directory; any changed query or setting fails.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ExternalBfs {
    pub discs: u32,
    pub jobs: usize,
    pub verbose: u8,
    pub block_size: usize,
    pub tmp_dir: PathBuf,
    pub resume: bool,
}
