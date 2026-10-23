//! Disk-backed stone-layer BFS with portable, query-bound checkpoints.
mod config;
mod expand;
mod io;
mod merge;
mod process;
mod search;

pub use config::Cfg;
pub use search::{parallel_retrospective_bfs, sequential_retrospective_bfs};
