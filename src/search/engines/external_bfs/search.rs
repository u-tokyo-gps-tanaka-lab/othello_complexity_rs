//! External breadth-first search and checkpoint resumption.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{
    expand::expand_batch,
    io::{
        contains_position, digest, identity, invalid, read_json, save_json, sync_parent,
        PositionReader, PositionWriter,
    },
    merge::{merge_sorted_bins, write_run},
    ExternalBfs,
};
use crate::othello::Position;
use crate::record::{PlyAction, ReachabilityRecord};
use crate::search::{
    forward::ForwardTable,
    moves::{pass_predecessor, successors},
    outcome::{found, SearchNode, SearchOutcome},
};

/// Sequential external-memory breadth-first search, retaining each exact
/// position in a sorted stone layer. Passes stay in that layer, and the search
/// has no node-count cutoff. `resume` validates every committed layer.
pub fn sequential_retrospective_bfs(
    cfg: &ExternalBfs,
    goal: Position,
    table: &ForwardTable,
) -> io::Result<SearchOutcome> {
    search(cfg, goal, table, false)
}

/// Parallel external BFS expands bounded batches and joins every worker before
/// committing a layer; the saved query and settings must match on resumption.
pub fn parallel_retrospective_bfs(
    cfg: &ExternalBfs,
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

fn layer_counts(value: &Value) -> io::Result<Vec<u64>> {
    value
        .get("layer_counts")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("missing or invalid checkpoint layer_counts"))?
        .iter()
        .map(|count| {
            count
                .as_u64()
                .ok_or_else(|| invalid("invalid checkpoint layer count"))
        })
        .collect()
}

fn layer_digests(value: &Value) -> io::Result<Vec<String>> {
    value
        .get("layer_sha256")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("missing or invalid checkpoint layer_sha256"))?
        .iter()
        .map(|digest| {
            digest
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid("invalid checkpoint layer digest"))
        })
        .collect()
}

fn checkpoint(
    path: &Path,
    context: &Value,
    counts: &[u64],
    digests: &[String],
    nodes: usize,
    outcome: Option<&SearchOutcome>,
) -> io::Result<()> {
    if counts.is_empty() || counts.len() != digests.len() {
        return Err(invalid("inconsistent external BFS layer metadata"));
    }
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
        &json!({ "identity": context, "layer": counts.len() - 1, "nodes": nodes,
        "layer_counts": counts, "layer_sha256": digests,
        "state": state, "record": record }),
    )
}

fn search(
    cfg: &ExternalBfs,
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
    if table.discs() != cfg.discs.min(goal.board.popcount()) || table.start() != Position::initial()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "forward table does not match external search settings",
        ));
    }
    let context = identity(cfg, goal, table.goal_match(), parallel);
    let state_path = cfg.tmp_dir.join("checkpoint.json");
    let mut layer = 0;
    let mut nodes = 0;
    let mut counts = Vec::new();
    let mut digests = Vec::new();
    fs::create_dir_all(&cfg.tmp_dir)?;
    let resume = cfg.resume && state_path.try_exists()?;
    if cfg.resume && !resume {
        clear_incomplete_initial_layer(&cfg.tmp_dir)?;
    } else if !cfg.resume {
        validate_fresh_directory(&cfg.tmp_dir)?;
    }
    if resume {
        let saved = read_json(&state_path)?;
        if saved.get("identity") != Some(&context) {
            return Err(invalid("checkpoint query or search settings mismatch"));
        }
        layer = unsigned(&saved, "layer")?;
        nodes = unsigned(&saved, "nodes")?;
        if layer > goal.board.popcount() as usize - table.discs() as usize {
            return Err(invalid("checkpoint progress exceeds search bounds"));
        }
        counts = layer_counts(&saved)?;
        digests = layer_digests(&saved)?;
        let expected_layers = layer
            .checked_add(1)
            .ok_or_else(|| invalid("checkpoint layer count overflow"))?;
        if counts.len() != expected_layers || digests.len() != expected_layers {
            return Err(invalid("checkpoint layer metadata length mismatch"));
        }
        let saved_record = saved
            .get("record")
            .ok_or_else(|| invalid("missing checkpoint record field"))?;
        let saved_outcome = match saved.get("state").and_then(Value::as_str) {
            Some("searching") => {
                if !saved_record.is_null() {
                    return Err(invalid("unfinished checkpoint contains a result record"));
                }
                None
            }
            Some("found") => {
                let record = ReachabilityRecord::from_csv(
                    saved_record
                        .as_str()
                        .ok_or_else(|| invalid("missing checkpoint certificate"))?,
                )
                .map_err(invalid)?;
                record.verify().map_err(invalid)?;
                if record.start != Position::initial()
                    || record.goal != goal
                    || record.goal_match != table.goal_match()
                {
                    return Err(invalid("checkpoint certificate belongs to another query"));
                }
                Some(SearchOutcome::Found { record, nodes })
            }
            Some("not-found") => {
                if !saved_record.is_null() {
                    return Err(invalid("unsuccessful checkpoint contains a certificate"));
                }
                Some(SearchOutcome::NotFound { nodes })
            }
            _ => return Err(invalid("invalid checkpoint completion state")),
        };
        validate_layers(&cfg.tmp_dir, &context, goal, &counts, &digests)?;
        cleanup_after_layer(&cfg.tmp_dir, layer)?;
        if let Some(outcome) = saved_outcome {
            return Ok(outcome);
        }
    } else {
        let layer_path = cfg.tmp_dir.join("layer_0.bin");
        let pending_path = layer_path.with_extension("pending");
        let mut first = vec![goal];
        if let Some(position) = pass_predecessor(goal) {
            first.push(position);
        }
        first.sort_unstable();
        first.dedup();
        let mut writer = PositionWriter::create(&pending_path, &context, 0)?;
        for position in first {
            writer.push(&position)?;
        }
        counts.push(writer.finish_synced()?);
        fs::rename(&pending_path, &layer_path)?;
        sync_parent(&layer_path)?;
        digests.push(digest(&layer_path)?);
        checkpoint(&state_path, &context, &counts, &digests, 0, None)?;
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
        let mut reader = PositionReader::open(&layer_path, &context, layer, goal)?;
        let mut batch = Vec::new();
        let mut runs = Vec::new();
        while let Some(position) = reader.next()? {
            if table.contains(position) {
                let node = meeting_node(&cfg.tmp_dir, &context, goal, position, layer)?;
                let outcome = found(table, goal, &node, nodes)?;
                checkpoint(
                    &state_path,
                    &context,
                    &counts,
                    &digests,
                    nodes,
                    Some(&outcome),
                )?;
                return Ok(outcome);
            }
            if position.board.popcount() <= table.discs() {
                continue;
            }
            nodes = nodes
                .checked_add(1)
                .ok_or_else(|| invalid("expanded-node count overflow"))?;
            batch.push(position);
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
                &counts,
                &digests,
                nodes,
                Some(&outcome),
            )?;
            return Ok(outcome);
        }
        let next_path = cfg.tmp_dir.join(format!("layer_{}.bin", layer + 1));
        let pending_path = next_path.with_extension("pending");
        let count = merge_sorted_bins(&runs, &pending_path, &context, layer + 1, goal)?;
        fs::rename(&pending_path, &next_path)?;
        sync_parent(&next_path)?;
        layer += 1;
        counts.push(count);
        digests.push(digest(&next_path)?);
        if cfg.verbose > 0 {
            eprintln!("reverse layer {layer}: {count} exact positions, {nodes} expansions");
        }
        let outcome = (count == 0).then_some(SearchOutcome::NotFound { nodes });
        checkpoint(
            &state_path,
            &context,
            &counts,
            &digests,
            nodes,
            outcome.as_ref(),
        )?;
        for path in runs {
            fs::remove_file(path)?;
        }
        if let Some(outcome) = outcome {
            return Ok(outcome);
        }
    }
}

fn validate_layers(
    directory: &Path,
    context: &Value,
    goal: Position,
    counts: &[u64],
    digests: &[String],
) -> io::Result<()> {
    for (layer, (&expected_count, expected_digest)) in counts.iter().zip(digests).enumerate() {
        let path = directory.join(format!("layer_{layer}.bin"));
        if !path.try_exists()? {
            return Err(invalid(format!("checkpoint layer {layer} is missing")));
        }
        if digest(&path)? != *expected_digest {
            return Err(invalid(format!("checkpoint layer {layer} digest mismatch")));
        }
        let mut reader = PositionReader::open(&path, context, layer, goal)?;
        if reader.count() != expected_count {
            return Err(invalid(format!(
                "checkpoint layer {layer} record count mismatch"
            )));
        }
        while reader.next()?.is_some() {}
    }
    Ok(())
}

pub(super) fn validate_fresh_directory(directory: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_dir() && fs::read_dir(directory)?.next().transpose()?.is_none() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "external search directory is not empty; choose a new directory or use --resume",
    ))
}

fn clear_incomplete_initial_layer(directory: &Path) -> io::Result<()> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let allowed = matches!(
            name.to_str(),
            Some("layer_0.bin" | "layer_0.pending" | "checkpoint.pending")
        );
        if !allowed || !entry.file_type()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "checkpoint is missing and the external search directory contains an unexpected entry",
            ));
        }
        paths.push(entry.path());
    }
    for path in paths {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn cleanup_after_layer(directory: &Path, layer: usize) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let stale_layer = parse_layer_file(name, ".bin").is_some_and(|index| index > layer);
        let pending_layer = parse_layer_file(name, ".pending").is_some();
        if stale_layer
            || pending_layer
            || parse_run_file(name).is_some()
            || name == "checkpoint.pending"
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn parse_layer_file(name: &str, suffix: &str) -> Option<usize> {
    parse_index(name.strip_prefix("layer_")?.strip_suffix(suffix)?)
}

fn parse_run_file(name: &str) -> Option<(usize, usize)> {
    let indices = name.strip_prefix("run_")?.strip_suffix(".bin")?;
    let (depth, run) = indices.split_once('_')?;
    Some((parse_index(depth)?, parse_index(run)?))
}

fn parse_index(value: &str) -> Option<usize> {
    if value.is_empty()
        || value.bytes().any(|byte| !byte.is_ascii_digit())
        || value.len() > 1 && value.starts_with('0')
    {
        return None;
    }
    value.parse().ok()
}

fn meeting_node(
    directory: &Path,
    context: &Value,
    goal: Position,
    meeting: Position,
    layer: usize,
) -> io::Result<SearchNode> {
    let mut position = meeting;
    let mut layer = layer;
    let mut actions = Vec::new();
    while position != goal {
        let mut selected = None;
        for (next, action) in successors(position) {
            let next_layer = match action {
                PlyAction::Play(_) => {
                    let Some(previous_layer) = layer.checked_sub(1) else {
                        continue;
                    };
                    previous_layer
                }
                PlyAction::Pass => layer,
            };
            let path = directory.join(format!("layer_{next_layer}.bin"));
            if contains_position(&path, context, next_layer, goal, next)? {
                selected = Some((next, action, next_layer));
                break;
            }
        }
        let Some((next, action, next_layer)) = selected else {
            return Err(invalid(
                "reverse-search layers contain no path from the meeting position to the goal",
            ));
        };
        position = next;
        layer = next_layer;
        actions.push(action);
    }
    actions.reverse();
    Ok(SearchNode::from_reverse_actions(meeting, actions))
}

fn flush_batch(
    cfg: &ExternalBfs,
    context: &Value,
    depth: usize,
    batch: &mut Vec<Position>,
    runs: &mut Vec<PathBuf>,
    pool: Option<&rayon::ThreadPool>,
) -> io::Result<()> {
    let children = expand_batch(batch, pool)?;
    let path = cfg.tmp_dir.join(format!("run_{depth}_{}.bin", runs.len()));
    write_run(&path, context, depth, children)?;
    runs.push(path);
    batch.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};

    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::othello::{BoardSymmetry, Color};
    use crate::record::GoalMatch;
    use crate::search::{
        constraints::viable, moves::predecessors, outcome::SearchResult,
        test_support::TemporaryDirectory,
    };

    fn config(tmp_dir: PathBuf, jobs: usize, block_size: usize, resume: bool) -> ExternalBfs {
        ExternalBfs {
            jobs,
            verbose: 0,
            block_size,
            discs: 4,
            tmp_dir,
            resume,
        }
    }

    fn opening_position(discs: u32) -> Position {
        let mut position = Position::initial();
        while position.board.popcount() < discs {
            position = successors(position)
                .into_iter()
                .next()
                .expect("the fixed opening reaches the requested stone count")
                .0;
        }
        position
    }

    fn pass_trace() -> (Vec<PlyAction>, Vec<Position>) {
        let mut rng = rand::rngs::StdRng::seed_from_u64(32);
        let mut actions = Vec::new();
        let mut positions = vec![Position::initial()];
        while actions.len() < 10 {
            let next = successors(*positions.last().unwrap());
            let (position, action) = next[rng.random_range(0..next.len())];
            positions.push(position);
            actions.push(action);
        }
        assert_eq!(actions[8], PlyAction::Pass);
        (actions, positions)
    }

    fn read_positions(path: &Path, context: &Value, depth: usize, goal: Position) -> Vec<Position> {
        let mut reader = PositionReader::open(path, context, depth, goal).unwrap();
        let mut positions = Vec::new();
        while let Some(position) = reader.next().unwrap() {
            positions.push(position);
        }
        positions
    }

    fn assert_verified(outcome: &SearchOutcome) {
        if let SearchOutcome::Found { record, .. } = outcome {
            record.verify().unwrap();
        }
    }

    fn assert_same_decision_and_nodes(left: &SearchOutcome, right: &SearchOutcome) {
        assert_eq!(left.result(), right.result());
        assert_eq!(left.nodes(), right.nodes());
        assert_verified(left);
        assert_verified(right);
    }

    #[test]
    fn saved_layers_follow_the_search_predecessor_filters_through_the_meeting_depth() {
        let directory = TemporaryDirectory::new("layer-oracle");
        let goal = opening_position(7);
        let cfg = config(directory.join("search"), 1, 2, false);
        let table = ForwardTable::new(4, Some(goal)).unwrap();
        let outcome = sequential_retrospective_bfs(&cfg, goal, &table).unwrap();
        assert!(matches!(outcome, SearchOutcome::Found { .. }));
        assert_verified(&outcome);

        let context = identity(&cfg, goal, GoalMatch::Exact, false);
        let meeting_depth = (goal.board.popcount() - table.discs()) as usize;
        let mut expected = BTreeSet::from([goal]);
        for (position, action) in predecessors(goal) {
            if action == PlyAction::Pass {
                expected.insert(position);
            }
        }
        for depth in 0..=meeting_depth {
            let actual: BTreeSet<_> = read_positions(
                &cfg.tmp_dir.join(format!("layer_{depth}.bin")),
                &context,
                depth,
                goal,
            )
            .into_iter()
            .collect();
            assert_eq!(actual, expected, "depth={depth}");
            if depth == meeting_depth {
                break;
            }
            let mut next = BTreeSet::new();
            for position in expected {
                for (previous, action) in predecessors(position) {
                    if !matches!(action, PlyAction::Play(_)) || !viable(previous).unwrap() {
                        continue;
                    }
                    next.insert(previous);
                    for (before_pass, pass) in predecessors(previous) {
                        if pass == PlyAction::Pass {
                            next.insert(before_pass);
                        }
                    }
                }
            }
            expected = next;
        }
    }

    #[test]
    fn reconstruction_handles_a_pass_in_the_first_layer() {
        let directory = TemporaryDirectory::new("first-layer-pass");
        let (_, trace) = pass_trace();
        let before_pass = trace[8];
        let goal = trace[9];
        assert_eq!(successors(before_pass), [(goal, PlyAction::Pass)]);
        let cfg = ExternalBfs {
            discs: goal.board.popcount(),
            tmp_dir: directory.join("search"),
            ..config(PathBuf::new(), 1, 4, false)
        };
        fs::create_dir(&cfg.tmp_dir).unwrap();
        let context = identity(&cfg, goal, GoalMatch::Exact, false);
        let path = cfg.tmp_dir.join("layer_0.bin");
        let mut first = vec![goal, before_pass];
        first.sort_unstable();
        let mut writer = PositionWriter::create(&path, &context, 0).unwrap();
        for position in first {
            writer.push(&position).unwrap();
        }
        writer.finish_synced().unwrap();

        let node = meeting_node(&cfg.tmp_dir, &context, goal, before_pass, 0).unwrap();
        assert_eq!(node.position, before_pass);
        let table = ForwardTable::new(goal.board.popcount(), Some(goal)).unwrap();
        let outcome = found(&table, goal, &node, 0).unwrap();
        let SearchOutcome::Found { record, .. } = outcome else {
            panic!("expected a reconstructed record");
        };
        assert_eq!(record.actions.last(), Some(&PlyAction::Pass));
        record.verify().unwrap();
    }

    #[test]
    fn exact_and_equivalence_records_include_actions_around_a_pass() {
        let directory = TemporaryDirectory::new("pass-records");
        let (_, trace) = pass_trace();
        let exact_goal = trace[10];
        let exact_cfg = ExternalBfs {
            discs: 10,
            tmp_dir: directory.join("exact"),
            ..config(PathBuf::new(), 1, 3, false)
        };
        let exact_table = ForwardTable::new(10, Some(exact_goal)).unwrap();
        let exact = sequential_retrospective_bfs(&exact_cfg, exact_goal, &exact_table).unwrap();

        let equivalent_goal = BoardSymmetry::MirrorHorizontal.apply_position(exact_goal);
        let equivalent_cfg = ExternalBfs {
            discs: 10,
            tmp_dir: directory.join("equivalence"),
            ..config(PathBuf::new(), 2, 2, false)
        };
        let equivalent_table = ForwardTable::for_equivalence(10, equivalent_goal).unwrap();
        let equivalent =
            parallel_retrospective_bfs(&equivalent_cfg, equivalent_goal, &equivalent_table)
                .unwrap();

        for (outcome, goal_match) in [
            (&exact, GoalMatch::Exact),
            (&equivalent, GoalMatch::EquivalenceClass),
        ] {
            let SearchOutcome::Found { record, .. } = outcome else {
                panic!("reachable pass query was rejected: {outcome:?}");
            };
            let pass = record
                .actions
                .iter()
                .position(|action| *action == PlyAction::Pass)
                .expect("the absolute turn requires a pass");
            assert!(record.actions[..pass]
                .iter()
                .any(|action| matches!(action, PlyAction::Play(_))));
            assert!(record.actions[pass + 1..]
                .iter()
                .any(|action| matches!(action, PlyAction::Play(_))));
            assert_eq!(record.goal_match, goal_match);
            record.verify().unwrap();
        }
    }

    #[test]
    fn sequential_parallel_jobs_and_block_sizes_preserve_decision_and_nodes() {
        let directory = TemporaryDirectory::new("search-settings");
        let goal = opening_position(7);
        let table = ForwardTable::new(4, Some(goal)).unwrap();
        let baseline_cfg = config(directory.join("sequential-1"), 1, 1, false);
        let baseline = sequential_retrospective_bfs(&baseline_cfg, goal, &table).unwrap();

        let sequential = sequential_retrospective_bfs(
            &config(directory.join("sequential-8"), 1, 8, false),
            goal,
            &table,
        )
        .unwrap();
        assert_same_decision_and_nodes(&baseline, &sequential);
        for (jobs, block_size) in [(1, 1), (2, 2), (4, 11)] {
            let parallel = parallel_retrospective_bfs(
                &config(
                    directory.join(format!("parallel-{jobs}-{block_size}")),
                    jobs,
                    block_size,
                    false,
                ),
                goal,
                &table,
            )
            .unwrap();
            assert_same_decision_and_nodes(&baseline, &parallel);
        }
    }

    #[test]
    fn resume_without_a_checkpoint_rejects_unexpected_entries_without_deleting_files() {
        let directory = TemporaryDirectory::new("resume-with-unexpected-entry");
        let tmp_dir = directory.join("search");
        fs::create_dir(&tmp_dir).unwrap();
        let incomplete_layer = tmp_dir.join("layer_0.pending");
        let user_file = tmp_dir.join("notes.txt");
        fs::write(&incomplete_layer, b"incomplete layer").unwrap();
        fs::write(&user_file, b"user data").unwrap();
        let goal = opening_position(7);
        let table = ForwardTable::new(4, Some(goal)).unwrap();

        let error =
            sequential_retrospective_bfs(&config(tmp_dir, 1, 2, true), goal, &table).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(incomplete_layer).unwrap(), b"incomplete layer");
        assert_eq!(fs::read(user_file).unwrap(), b"user data");
    }

    #[test]
    fn resume_from_an_older_layer_discards_uncommitted_files() {
        let directory = TemporaryDirectory::new("resume-layer");
        let goal = opening_position(7);
        let tmp_dir = directory.join("search");
        let cfg = config(tmp_dir.clone(), 1, 2, false);
        let table = ForwardTable::new(4, Some(goal)).unwrap();
        let first = sequential_retrospective_bfs(&cfg, goal, &table).unwrap();
        let checkpoint_path = tmp_dir.join("checkpoint.json");
        let mut checkpoint = read_json(&checkpoint_path).unwrap();
        let final_layer = unsigned(&checkpoint, "layer").unwrap();
        assert!(final_layer >= 2);
        let first_count = checkpoint["layer_counts"][0].as_u64().unwrap();
        checkpoint["layer"] = json!(1);
        checkpoint["nodes"] = json!(first_count);
        checkpoint["state"] = json!("searching");
        checkpoint["record"] = Value::Null;
        checkpoint["layer_counts"]
            .as_array_mut()
            .unwrap()
            .truncate(2);
        checkpoint["layer_sha256"]
            .as_array_mut()
            .unwrap()
            .truncate(2);
        save_json(&checkpoint_path, &checkpoint).unwrap();
        fs::write(tmp_dir.join("layer_99.bin"), b"stale layer").unwrap();
        fs::write(tmp_dir.join("layer_2.pending"), b"stale pending").unwrap();
        fs::write(tmp_dir.join("run_2_99.bin"), b"stale run").unwrap();
        fs::write(tmp_dir.join("checkpoint.pending"), b"stale checkpoint").unwrap();
        fs::write(tmp_dir.join("notes.pending"), b"user notes").unwrap();
        fs::write(tmp_dir.join("layer_1.bin.bak"), b"user backup").unwrap();
        fs::create_dir(tmp_dir.join("run_notes")).unwrap();
        fs::write(tmp_dir.join("run_notes/notes.txt"), b"user notes").unwrap();

        let resumed =
            sequential_retrospective_bfs(&config(tmp_dir.clone(), 1, 2, true), goal, &table)
                .unwrap();
        assert_same_decision_and_nodes(&first, &resumed);
        assert!(!tmp_dir.join("layer_99.bin").exists());
        assert!(!tmp_dir.join("layer_2.pending").exists());
        assert!(!tmp_dir.join("run_2_99.bin").exists());
        assert!(!tmp_dir.join("checkpoint.pending").exists());
        assert_eq!(
            fs::read(tmp_dir.join("notes.pending")).unwrap(),
            b"user notes"
        );
        assert_eq!(
            fs::read(tmp_dir.join("layer_1.bin.bak")).unwrap(),
            b"user backup"
        );
        assert_eq!(
            fs::read(tmp_dir.join("run_notes/notes.txt")).unwrap(),
            b"user notes"
        );
    }

    #[test]
    fn completed_found_and_not_found_searches_validate_then_resume() {
        let directory = TemporaryDirectory::new("completed-resume");
        let found_goal = opening_position(7);
        let found_table = ForwardTable::new(4, Some(found_goal)).unwrap();
        let found_dir = directory.join("found");
        let first_found = sequential_retrospective_bfs(
            &config(found_dir.clone(), 1, 3, false),
            found_goal,
            &found_table,
        )
        .unwrap();
        let resumed_found =
            sequential_retrospective_bfs(&config(found_dir, 1, 3, true), found_goal, &found_table)
                .unwrap();
        assert_same_decision_and_nodes(&first_found, &resumed_found);

        let not_found_goal = Position {
            side_to_move: Color::White,
            ..Position::initial()
        };
        let not_found_table = ForwardTable::new(4, Some(not_found_goal)).unwrap();
        let not_found_dir = directory.join("not-found");
        let first_not_found = sequential_retrospective_bfs(
            &config(not_found_dir.clone(), 1, 3, false),
            not_found_goal,
            &not_found_table,
        )
        .unwrap();
        assert_eq!(first_not_found.result(), SearchResult::NotFound);
        let resumed_not_found = sequential_retrospective_bfs(
            &config(not_found_dir, 1, 3, true),
            not_found_goal,
            &not_found_table,
        )
        .unwrap();
        assert_same_decision_and_nodes(&first_not_found, &resumed_not_found);
    }

    fn completed_search(name: &str) -> (TemporaryDirectory, ExternalBfs, Position, ForwardTable) {
        let directory = TemporaryDirectory::new(name);
        let goal = opening_position(7);
        let cfg = config(directory.join("search"), 1, 2, false);
        let table = ForwardTable::new(4, Some(goal)).unwrap();
        sequential_retrospective_bfs(&cfg, goal, &table).unwrap();
        (directory, cfg, goal, table)
    }

    #[test]
    fn resume_rejects_corrupt_saved_layers_and_checkpoint_counts() {
        #[derive(Clone, Copy)]
        enum Corruption {
            Digest,
            Missing,
            Order,
            Count,
        }

        for (name, corruption, expected_message) in [
            (
                "digest-mismatch",
                Corruption::Digest,
                "layer 0 digest mismatch",
            ),
            ("missing-layer", Corruption::Missing, "layer 0 is missing"),
            (
                "record-order",
                Corruption::Order,
                "strictly increasing order",
            ),
            ("count-mismatch", Corruption::Count, "record count mismatch"),
        ] {
            let (_directory, cfg, goal, table, protected_file) = match corruption {
                Corruption::Digest => {
                    let (directory, cfg, goal, table) = completed_search(name);
                    let mut file = OpenOptions::new()
                        .append(true)
                        .open(cfg.tmp_dir.join("layer_0.bin"))
                        .unwrap();
                    file.write_all(&[0]).unwrap();
                    let uncommitted_run = cfg.tmp_dir.join("run_9_0.bin");
                    fs::write(&uncommitted_run, b"uncommitted run").unwrap();
                    (directory, cfg, goal, table, Some(uncommitted_run))
                }
                Corruption::Missing => {
                    let (directory, cfg, goal, table) = completed_search(name);
                    fs::remove_file(cfg.tmp_dir.join("layer_0.bin")).unwrap();
                    (directory, cfg, goal, table, None)
                }
                Corruption::Order => {
                    let directory = TemporaryDirectory::new(name);
                    let (_, trace) = pass_trace();
                    let goal = trace[10];
                    let cfg = ExternalBfs {
                        discs: 10,
                        tmp_dir: directory.join("search"),
                        ..config(PathBuf::new(), 1, 2, false)
                    };
                    let table = ForwardTable::new(10, Some(goal)).unwrap();
                    sequential_retrospective_bfs(&cfg, goal, &table).unwrap();
                    let checkpoint_path = cfg.tmp_dir.join("checkpoint.json");
                    let mut checkpoint = read_json(&checkpoint_path).unwrap();
                    let counts = checkpoint["layer_counts"].as_array().unwrap();
                    let layer = counts
                        .iter()
                        .position(|count| count.as_u64().unwrap() >= 2)
                        .expect("the shallow search retains a multi-position layer");
                    let path = cfg.tmp_dir.join(format!("layer_{layer}.bin"));
                    let mut file = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .unwrap();
                    file.seek(SeekFrom::Start(20)).unwrap();
                    let mut length = [0_u8; 4];
                    std::io::Read::read_exact(&mut file, &mut length).unwrap();
                    let header_length = 24 + u32::from_le_bytes(length) as u64;
                    file.seek(SeekFrom::Start(header_length)).unwrap();
                    let mut records = [0_u8; 34];
                    std::io::Read::read_exact(&mut file, &mut records).unwrap();
                    let (first, second) = records.split_at_mut(17);
                    first.swap_with_slice(second);
                    file.seek(SeekFrom::Start(header_length)).unwrap();
                    file.write_all(&records).unwrap();
                    drop(file);
                    checkpoint["layer_sha256"][layer] = json!(digest(&path).unwrap());
                    save_json(&checkpoint_path, &checkpoint).unwrap();
                    (directory, cfg, goal, table, None)
                }
                Corruption::Count => {
                    let (directory, cfg, goal, table) = completed_search(name);
                    let checkpoint_path = cfg.tmp_dir.join("checkpoint.json");
                    let mut checkpoint = read_json(&checkpoint_path).unwrap();
                    let count = checkpoint["layer_counts"][0].as_u64().unwrap();
                    checkpoint["layer_counts"][0] = json!(count + 1);
                    save_json(&checkpoint_path, &checkpoint).unwrap();
                    (directory, cfg, goal, table, None)
                }
            };

            let error = sequential_retrospective_bfs(
                &ExternalBfs {
                    resume: true,
                    ..cfg
                },
                goal,
                &table,
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{name}");
            assert!(
                error.to_string().contains(expected_message),
                "{name}: {error}"
            );
            if let Some(path) = protected_file {
                assert_eq!(fs::read(path).unwrap(), b"uncommitted run", "{name}");
            }
        }
    }

    #[test]
    fn external_search_rejects_a_table_with_another_boundary() {
        let directory = TemporaryDirectory::new("external-table-boundary");
        let goal = successors(Position::initial())[0].0;
        let table = ForwardTable::for_equivalence(4, goal).unwrap();
        let tmp_dir = directory.join("tmp");
        let cfg = ExternalBfs {
            jobs: 1,
            verbose: 0,
            block_size: 8,
            discs: 5,
            tmp_dir: tmp_dir.clone(),
            resume: false,
        };

        let error = sequential_retrospective_bfs(&cfg, goal, &table).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!tmp_dir.exists());
    }
}
