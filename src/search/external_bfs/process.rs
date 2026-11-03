use super::io::PositionWriter;
use crate::othello::Position;
use crate::search::core::{append_placement_predecessors, pass_predecessor, viable};
use serde_json::Value;
use std::io;
use std::path::Path;

pub(super) fn process_node(position: &Position) -> io::Result<Vec<Position>> {
    let mut previous = Vec::new();
    append_placement_predecessors(*position, position.board.opponent, &mut previous);
    let mut children = Vec::new();
    for (position, _) in previous {
        // External BFS always applies LP pruning to each generated placement.
        if !viable(position, true, false)? {
            continue;
        }
        let pass = pass_predecessor(position);
        children.push(position);
        children.extend(pass);
    }
    Ok(children)
}

pub(super) fn write_run(
    path: &Path,
    context: &Value,
    depth: usize,
    mut positions: Vec<Position>,
) -> io::Result<()> {
    positions.sort_unstable();
    positions.dedup();
    let mut writer = PositionWriter::create(path, context, depth)?;
    for position in positions {
        writer.push(&position)?;
    }
    writer.finish()?;
    Ok(())
}
