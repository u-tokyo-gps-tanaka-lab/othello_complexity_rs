use super::process::process_node;
use crate::search::core::SearchNode;
use rayon::prelude::*;
use std::io;

/// Parallel expansion is confined to a bounded input batch. Its barrier joins
/// all jobs before the layer can be committed or declared empty.
pub(super) fn expand_batch(
    nodes: &[SearchNode],
    pool: Option<&rayon::ThreadPool>,
) -> io::Result<Vec<SearchNode>> {
    let batches: io::Result<Vec<Vec<SearchNode>>> = match pool {
        Some(pool) => pool.install(|| nodes.par_iter().map(process_node).collect()),
        None => nodes.iter().map(process_node).collect(),
    };
    Ok(batches?.into_iter().flatten().collect())
}
