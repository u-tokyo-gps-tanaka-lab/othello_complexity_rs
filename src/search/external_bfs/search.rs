use std::fs;
use std::io;
use std::path::Path;

use serde_json::{json, Value};

use super::{
    config::Cfg,
    expand::expand_batch,
    io::{digest, identity, invalid, read_json, save_json, NodeReader, NodeWriter},
    merge::merge_sorted_bins,
    process::write_run,
};
use crate::othello::Position;
use crate::record::{PlyAction, ReachabilityRecord};
use crate::search::{
    core::{found, pass_predecessor, SearchNode, SearchOutcome},
    forward::ForwardTable,
};

/// Sequential external-memory breadth-first search, retaining one suffix per
/// exact position in each stone layer. Passes stay in that layer, and the search
/// has no node-count cutoff. `resume` loads its committed layer.
pub fn sequential_retrospective_bfs(
    cfg: &Cfg,
    goal: Position,
    table: &ForwardTable,
) -> io::Result<SearchOutcome> {
    search(cfg, goal, table, false)
}

/// Parallel external BFS expands bounded batches and joins every worker before
/// committing a layer; the saved query and settings must match on resumption.
pub fn parallel_retrospective_bfs(
    cfg: &Cfg,
    goal: Position,
    table: &ForwardTable,
) -> io::Result<SearchOutcome> {
    search(cfg, goal, table, true)
}

fn unsigned(value: &Value, field: &str) -> io::Result<usize> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| invalid(format!("missing or invalid checkpoint {field}")))
}

fn checkpoint(
    path: &Path,
    context: &Value,
    layer: usize,
    nodes: usize,
    layer_path: &Path,
    outcome: Option<&SearchOutcome>,
) -> io::Result<()> {
    let (state, record) = match outcome {
        None => ("searching", Value::Null),
        Some(SearchOutcome::Found { record, .. }) => ("found", Value::String(record.to_csv())),
        Some(SearchOutcome::NotFound { .. }) => ("not-found", Value::Null),
        Some(SearchOutcome::Unknown { .. }) => {
            return Err(invalid(
                "unbounded external BFS cannot save a resource stop",
            ));
        }
    };
    save_json(
        path,
        &json!({ "identity": context, "layer": layer, "nodes": nodes,
        "layer_sha256": digest(layer_path)?, "state": state, "record": record }),
    )
}

fn search(
    cfg: &Cfg,
    goal: Position,
    table: &ForwardTable,
    parallel: bool,
) -> io::Result<SearchOutcome> {
    table.validate_goal(goal)?;
    if !(4..=64).contains(&cfg.discs) || cfg.block_size == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "discs must be 4..64 and block-size must be positive",
        ));
    }
    if table.discs() != (cfg.discs as u32).min(goal.board.popcount())
        || table.start() != Position::initial()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "forward table does not match external search settings",
        ));
    }
    fs::create_dir_all(&cfg.tmp_dir)?;
    let context = identity(cfg, goal, parallel);
    let state_path = cfg.tmp_dir.join("checkpoint.json");
    let mut layer = 0;
    let mut nodes = 0;
    if cfg.resume {
        let saved = read_json(&state_path)?;
        if saved.get("identity") != Some(&context) {
            return Err(invalid("checkpoint query or search settings mismatch"));
        }
        layer = unsigned(&saved, "layer")?;
        nodes = unsigned(&saved, "nodes")?;
        if layer > goal.board.popcount() as usize - table.discs() as usize {
            return Err(invalid("checkpoint progress exceeds search bounds"));
        }
        let layer_path = cfg.tmp_dir.join(format!("layer_{layer}.bin"));
        if saved.get("layer_sha256").and_then(Value::as_str) != Some(&digest(&layer_path)?) {
            return Err(invalid("checkpoint layer digest mismatch"));
        }
        // Validate every retained suffix, even if a saved result would let the
        // caller return before traversing the file.
        let mut reader = NodeReader::open(&layer_path, &context, layer, goal)?;
        while reader.next()?.is_some() {}
        let saved_record = saved
            .get("record")
            .ok_or_else(|| invalid("missing checkpoint record field"))?;
        match saved.get("state").and_then(Value::as_str) {
            Some("searching") => {
                if !saved_record.is_null() {
                    return Err(invalid("unfinished checkpoint contains a result record"));
                }
            }
            Some("found") => {
                let record = ReachabilityRecord::from_csv(
                    saved
                        .get("record")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid("missing checkpoint certificate"))?,
                )
                .map_err(invalid)?;
                record.verify().map_err(invalid)?;
                if record.start != Position::initial()
                    || record.goal != goal
                    || record.goal_match != crate::record::GoalMatch::Exact
                {
                    return Err(invalid("checkpoint certificate belongs to another query"));
                }
                return Ok(SearchOutcome::Found { record, nodes });
            }
            Some("not-found") => {
                if !saved_record.is_null() {
                    return Err(invalid("unsuccessful checkpoint contains a certificate"));
                }
                return Ok(SearchOutcome::NotFound { nodes });
            }
            _ => return Err(invalid("invalid checkpoint completion state")),
        }
    } else {
        if state_path.exists() || fs::read_dir(&cfg.tmp_dir)?.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "external search directory is not empty; choose a new directory or use --resume",
            ));
        }
        let layer_path = cfg.tmp_dir.join("layer_0.bin");
        let mut writer = NodeWriter::create(&layer_path, &context, 0)?;
        let root = SearchNode::root(goal);
        writer.push(&root)?;
        if let Some(position) = pass_predecessor(goal) {
            writer.push(&root.preceding(position, PlyAction::Pass))?;
        }
        writer.finish()?;
        checkpoint(&state_path, &context, 0, 0, &layer_path, None)?;
    }
    let pool = if parallel {
        Some(
            rayon::ThreadPoolBuilder::new()
                .num_threads(cfg.jobs)
                .build()
                .map_err(io::Error::other)?,
        )
    } else {
        None
    };
    loop {
        let layer_path = cfg.tmp_dir.join(format!("layer_{layer}.bin"));
        let mut reader = NodeReader::open(&layer_path, &context, layer, goal)?;
        let mut batch = Vec::new();
        let mut runs = Vec::new();
        while let Some(node) = reader.next()? {
            if table.contains(node.position) {
                let outcome = found(table, goal, &node, nodes)?;
                checkpoint(
                    &state_path,
                    &context,
                    layer,
                    nodes,
                    &layer_path,
                    Some(&outcome),
                )?;
                return Ok(outcome);
            }
            if node.position.board.popcount() <= table.discs() {
                continue;
            }
            nodes = nodes
                .checked_add(1)
                .ok_or_else(|| invalid("expanded-node count overflow"))?;
            batch.push(node);
            if batch.len() == cfg.block_size {
                flush_batch(
                    cfg,
                    &context,
                    layer + 1,
                    &mut batch,
                    &mut runs,
                    pool.as_ref(),
                )?;
            }
        }
        if !batch.is_empty() {
            flush_batch(
                cfg,
                &context,
                layer + 1,
                &mut batch,
                &mut runs,
                pool.as_ref(),
            )?;
        }
        if runs.is_empty() {
            let outcome = SearchOutcome::NotFound { nodes };
            checkpoint(
                &state_path,
                &context,
                layer,
                nodes,
                &layer_path,
                Some(&outcome),
            )?;
            return Ok(outcome);
        }
        let next_path = cfg.tmp_dir.join(format!("layer_{}.bin", layer + 1));
        let pending_path = next_path.with_extension("pending");
        let count = merge_sorted_bins(&runs, &pending_path, &context, layer + 1, goal)?;
        fs::rename(pending_path, &next_path)?;
        layer += 1;
        if cfg.verbose > 0 {
            eprintln!("reverse layer {layer}: {count} exact positions, {nodes} expansions");
        }
        let outcome = (count == 0).then_some(SearchOutcome::NotFound { nodes });
        checkpoint(
            &state_path,
            &context,
            layer,
            nodes,
            &next_path,
            outcome.as_ref(),
        )?;
        // Only scratch runs produced by this completed layer are removed.
        for path in runs {
            fs::remove_file(path)?;
        }
        if let Some(outcome) = outcome {
            return Ok(outcome);
        }
    }
}

fn flush_batch(
    cfg: &Cfg,
    context: &Value,
    depth: usize,
    batch: &mut Vec<SearchNode>,
    runs: &mut Vec<std::path::PathBuf>,
    pool: Option<&rayon::ThreadPool>,
) -> io::Result<()> {
    let children = expand_batch(batch, pool)?;
    let path = cfg.tmp_dir.join(format!("run_{depth}_{}.bin", runs.len()));
    write_run(&path, context, depth, children)?;
    runs.push(path);
    batch.clear();
    Ok(())
}
