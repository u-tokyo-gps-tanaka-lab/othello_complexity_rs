//! Disk-backed stone-layer BFS with portable, query-bound checkpoints.
mod config;
mod expand;
mod io;
mod merge;
mod process;
mod search;

pub use config::Cfg;
pub(crate) use io::sync_parent;
pub(crate) use search::validate_fresh_directory;
pub use search::{parallel_retrospective_bfs, sequential_retrospective_bfs};
