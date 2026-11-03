use super::process::process_node;
use crate::othello::Position;
use rayon::prelude::*;
use std::io;

/// Parallel expansion is confined to a bounded input batch. Its barrier joins
/// all jobs before the layer can be committed or declared empty.
pub(super) fn expand_batch(
    positions: &[Position],
    pool: Option<&rayon::ThreadPool>,
) -> io::Result<Vec<Position>> {
    let batches: io::Result<Vec<Vec<Position>>> = match pool {
        Some(pool) => pool.install(|| positions.par_iter().map(process_node).collect()),
        None => positions.iter().map(process_node).collect(),
    };
    Ok(batches?.into_iter().flatten().collect())
}
