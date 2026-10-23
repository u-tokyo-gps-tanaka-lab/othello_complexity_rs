use super::io::NodeWriter;
use crate::record::PlyAction;
use crate::search::core::{append_placement_predecessors, pass_predecessor, viable, SearchNode};
use serde_json::Value;
use std::io;
use std::path::Path;

pub(super) fn process_node(node: &SearchNode) -> io::Result<Vec<SearchNode>> {
    let mut previous = Vec::new();
    append_placement_predecessors(node.position, node.position.board.opponent, &mut previous);
    let mut children = Vec::new();
    for (position, action) in previous {
        // External BFS always applies LP pruning to each generated placement.
        if !viable(position, true, false)? {
            continue;
        }
        let child = node.preceding(position, action);
        let pass =
            pass_predecessor(position).map(|position| child.preceding(position, PlyAction::Pass));
        children.push(child);
        children.extend(pass);
    }
    Ok(children)
}

pub(super) fn write_run(
    path: &Path,
    context: &Value,
    depth: usize,
    mut nodes: Vec<SearchNode>,
) -> io::Result<()> {
    nodes.sort_by_key(|node| node.position);
    nodes.dedup_by_key(|node| node.position);
    let mut writer = NodeWriter::create(path, context, depth)?;
    for node in nodes {
        writer.push(&node)?;
    }
    writer.finish()?;
    Ok(())
}
