use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io;
use std::path::{Path, PathBuf};

use super::io::{PositionReader, PositionWriter};
use crate::othello::Position;
use serde_json::Value;

/// Merge sorted runs by the full position, retaining one record for each.
pub(super) fn merge_sorted_bins(
    paths: &[PathBuf],
    output: &Path,
    context: &Value,
    depth: usize,
    goal: Position,
) -> io::Result<u64> {
    let mut readers: Vec<_> = paths
        .iter()
        .map(|path| PositionReader::open(path, context, depth, goal))
        .collect::<io::Result<_>>()?;
    let mut heads = Vec::with_capacity(readers.len());
    let mut heap = BinaryHeap::new();
    for (index, reader) in readers.iter_mut().enumerate() {
        let head = reader.next()?;
        if let Some(position) = head {
            heap.push(Reverse((position, index)));
        }
        heads.push(head);
    }
    let mut writer = PositionWriter::create(output, context, depth)?;
    let mut previous = None;
    while let Some(Reverse((position, index))) = heap.pop() {
        heads[index].take().expect("merge head missing");
        if previous != Some(position) {
            writer.push(&position)?;
            previous = Some(position);
        }
        heads[index] = readers[index].next()?;
        if let Some(next) = heads[index] {
            heap.push(Reverse((next, index)));
        }
    }
    writer.finish_synced()
}
