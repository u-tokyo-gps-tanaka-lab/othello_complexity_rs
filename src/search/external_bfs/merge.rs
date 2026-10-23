use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io;
use std::path::{Path, PathBuf};

use super::io::{NodeReader, NodeWriter};
use crate::othello::Position;
use serde_json::Value;

/// Merge sorted runs by the full position, retaining one valid suffix for each.
pub(super) fn merge_sorted_bins(
    paths: &[PathBuf],
    output: &Path,
    context: &Value,
    depth: usize,
    goal: Position,
) -> io::Result<u64> {
    let mut readers: Vec<_> = paths
        .iter()
        .map(|path| NodeReader::open(path, context, depth, goal))
        .collect::<io::Result<_>>()?;
    let mut heads = Vec::with_capacity(readers.len());
    let mut heap = BinaryHeap::new();
    for (index, reader) in readers.iter_mut().enumerate() {
        let head = reader.next()?;
        if let Some(node) = &head {
            heap.push(Reverse((node.position, index)));
        }
        heads.push(head);
    }
    let mut writer = NodeWriter::create(output, context, depth)?;
    let mut previous = None;
    while let Some(Reverse((position, index))) = heap.pop() {
        let node = heads[index].take().expect("merge head missing");
        if previous != Some(position) {
            writer.push(&node)?;
            previous = Some(position);
        }
        heads[index] = readers[index].next()?;
        if let Some(next) = &heads[index] {
            heap.push(Reverse((next.position, index)));
        }
    }
    writer.finish()
}
