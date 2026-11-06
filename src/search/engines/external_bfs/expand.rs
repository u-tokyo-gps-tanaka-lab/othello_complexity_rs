//! Expansion of one external breadth-first search layer.

use crate::othello::Position;
use crate::search::{
    constraints::viable,
    moves::{append_placement_predecessors, pass_predecessor},
};
use rayon::prelude::*;
use std::io;

fn process_node(position: &Position) -> io::Result<Vec<Position>> {
    let mut previous = Vec::new();
    append_placement_predecessors(*position, position.board.opponent, &mut previous);
    let mut children = Vec::new();
    for (position, _) in previous {
        // External BFS always applies LP pruning to each generated placement.
        if !viable(position)? {
            continue;
        }
        let pass = pass_predecessor(position);
        children.push(position);
        children.extend(pass);
    }
    Ok(children)
}

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
