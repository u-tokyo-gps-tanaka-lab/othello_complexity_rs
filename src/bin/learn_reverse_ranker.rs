//! Generate replay-verified demonstrations for a reverse-search ranker.

use clap::Parser;
use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{BoardSymmetry, Position};
use othello_complexity_rs::record::{GoalMatch, PlyAction, PositionTransform, ReachabilityRecord};
use othello_complexity_rs::search::core::successors;
use othello_complexity_rs::search::forward::ForwardTable;
use othello_complexity_rs::search::learned::{features, FEATURE_COUNT, FEATURE_VERSION};
use othello_complexity_rs::search::learning_data::sample_training_group;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(about = "Generate observed-predecessor ranking groups, never reachability labels")]
struct Args {
    #[arg(long, default_value_t = 1_000)]
    games: usize,
    #[arg(long)]
    seed: u64,
    #[arg(long)]
    split: String,
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "12,16,20,24,28,32,36,40,44,48,52,56"
    )]
    stones: Vec<u32>,
    #[arg(long, default_value_t = 8)]
    discs: u32,
    #[arg(long, default_value_t = 7)]
    comparisons: usize,
    #[arg(long, default_value_t = 128)]
    max_draws: usize,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    records_out: PathBuf,
    #[arg(long)]
    metadata_out: PathBuf,
    /// Sorted canonical keys used in these groups, for split-overlap checking.
    #[arg(long)]
    keys_out: PathBuf,
    /// Exclude an entire group if any candidate or goal matches one of these keys.
    #[arg(long)]
    exclude_keys: Option<PathBuf>,
}

fn canonical(position: Position) -> Position {
    [
        BoardSymmetry::Identity,
        BoardSymmetry::Rotate180,
        BoardSymmetry::Transpose,
        BoardSymmetry::AntiTranspose,
    ]
    .into_iter()
    .map(|symmetry| {
        assert_eq!(
            symmetry.apply_position(Position::initial()),
            Position::initial()
        );
        symmetry.apply_position(position)
    })
    .min()
    .unwrap()
}

fn sha256(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut chunk = [0_u8; 65_536];
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        digest.update(&chunk[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn parent_dirs(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    if args.games == 0
        || args.comparisons == 0
        || args.max_draws == 0
        || args
            .stones
            .iter()
            .any(|&n| !(args.discs + 2..=64).contains(&n))
    {
        return Err(
            io::Error::other("invalid games, comparisons, draw limit, or stone counts").into(),
        );
    }
    let started = Instant::now();
    for path in [
        &args.output,
        &args.records_out,
        &args.metadata_out,
        &args.keys_out,
    ] {
        parent_dirs(path)?;
        if path.exists() {
            return Err(io::Error::other(format!("refusing to replace {}", path.display())).into());
        }
    }
    let excluded = if let Some(path) = &args.exclude_keys {
        BufReader::new(File::open(path)?)
            .lines()
            .collect::<io::Result<HashSet<_>>>()?
    } else {
        HashSet::new()
    };
    let table = ForwardTable::new(args.discs, None)?;
    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_path(&args.output)?;
    let mut header = vec![
        "split".to_owned(),
        "game_id".to_owned(),
        "group_id".to_owned(),
        "stones".to_owned(),
        "goal".to_owned(),
        "candidate".to_owned(),
        "observed".to_owned(),
        "canonical_goal".to_owned(),
        "canonical_candidate".to_owned(),
    ];
    header.extend((0..FEATURE_COUNT).map(|index| format!("f{index}")));
    writer.write_record(&header)?;
    let mut source = BufWriter::new(File::create(&args.records_out)?);
    let requested: HashSet<_> = args.stones.iter().copied().collect();
    let mut rng = ChaCha8Rng::seed_from_u64(args.seed);
    let mut sample_rng = ChaCha8Rng::seed_from_u64(args.seed ^ 0x6c65_6172_6e65_6401);
    let mut keys = BTreeSet::new();
    let mut rows = 0;
    let mut groups = 0;
    let mut observed_rows = 0;
    let mut groups_by_stones = std::collections::BTreeMap::<u32, usize>::new();
    let mut empty_observed_groups = 0;
    let mut no_comparison_groups = 0;
    let mut overlap_groups = 0;
    let mut draw_cap_groups = 0;
    let mut sampled_placements = 0;
    let mut possible_placements = 0;
    let mut examined_candidates = 0;
    let mut filtered_candidates = 0;
    let mut source_passes = 0;
    let mut terminal_before_max = 0;
    for game in 0..args.games {
        let mut position = Position::initial();
        let mut actions = Vec::new();
        loop {
            let next = successors(position);
            if next.is_empty() {
                break;
            }
            let (child, action) = next[rng.random_range(0..next.len())];
            position = child;
            actions.push(action);
        }
        let record = ReachabilityRecord {
            input_index: game + 1,
            start: Position::initial(),
            goal: position,
            goal_match: GoalMatch::Exact,
            transform: PositionTransform::IDENTITY,
            actions,
        };
        let trace = record.verify().map_err(io::Error::other)?;
        source_passes += record
            .actions
            .iter()
            .filter(|a| matches!(a, PlyAction::Pass))
            .count();
        if position.board.popcount() < *args.stones.iter().max().unwrap() {
            terminal_before_max += 1;
        }
        writeln!(source, "{}", record.to_csv())?;
        for (step, action) in record.actions.iter().enumerate() {
            let goal = trace[step + 1];
            let stones = goal.board.popcount();
            if !matches!(action, PlyAction::Play(_)) || !requested.contains(&stones) {
                continue;
            }
            let mut observed = Vec::new();
            let mut previous_index = step;
            loop {
                let previous = trace[previous_index];
                if previous.board.popcount() + 1 != stones {
                    break;
                }
                observed.push(previous);
                if previous_index == 0 {
                    break;
                }
                previous_index -= 1;
            }
            let group = sample_training_group(
                goal,
                &observed,
                &table,
                args.comparisons,
                args.max_draws,
                &mut sample_rng,
            )?;
            sampled_placements += group.sampled_placements;
            possible_placements += group.placement_combinations;
            examined_candidates += group.examined_candidates;
            filtered_candidates += group.filtered_candidates;
            draw_cap_groups += usize::from(group.draw_cap_reached);
            if !group.candidates.iter().any(|(_, known)| *known) {
                empty_observed_groups += 1;
                continue;
            }
            if !group.candidates.iter().any(|(_, known)| !*known) {
                no_comparison_groups += 1;
                continue;
            }
            let canonical_goal = format_position(canonical(goal));
            let candidate_keys: Vec<_> = group
                .candidates
                .iter()
                .map(|(candidate, _)| format_position(canonical(*candidate)))
                .collect();
            if excluded.contains(&canonical_goal)
                || candidate_keys.iter().any(|key| excluded.contains(key))
            {
                overlap_groups += 1;
                continue;
            }
            groups += 1;
            *groups_by_stones.entry(stones).or_default() += 1;
            keys.insert(canonical_goal.clone());
            let goal_text = format_position(goal);
            for ((candidate, known), candidate_key) in group.candidates.iter().zip(candidate_keys) {
                let mut row = vec![
                    args.split.clone(),
                    format!("{}:{}", args.seed, game),
                    format!("{}:{}:{}", args.seed, game, step),
                    stones.to_string(),
                    goal_text.clone(),
                    format_position(*candidate),
                    usize::from(*known).to_string(),
                    canonical_goal.clone(),
                    candidate_key.clone(),
                ];
                let encoded = features(*candidate);
                if encoded.iter().any(|value| !value.is_finite()) {
                    return Err(io::Error::other("nonfinite encoder output").into());
                }
                row.extend(encoded.into_iter().map(|value| value.to_string()));
                writer.write_record(&row)?;
                keys.insert(candidate_key);
                rows += 1;
                observed_rows += usize::from(*known);
            }
        }
        if (game + 1) % 100 == 0 {
            eprintln!(
                "{}: {} games, {groups} groups, {rows} rows",
                args.split,
                game + 1
            );
        }
    }
    writer.flush()?;
    source.flush()?;
    let mut key_file = BufWriter::new(File::create(&args.keys_out)?);
    for key in &keys {
        writeln!(key_file, "{key}")?;
    }
    key_file.flush()?;
    let metadata = json!({
        "schema": "othello-reverse-ranking-data-v1",
        "feature_version": FEATURE_VERSION,
        "feature_count": FEATURE_COUNT,
        "label_semantics": "observed=1 means a retainable predecessor on the verified source history; observed=0 is an unlabelled alternative, not an unreachability label",
        "sampling": "uniform placement predecessor indices without replacement, followed by their legal reverse-pass candidates; first accepted comparisons retained; no iterator-prefix sampling",
        "split": args.split,
        "seed": args.seed,
        "candidate_seed": args.seed ^ 0x6c65_6172_6e65_6401,
        "rng": "rand_chacha 0.9.0 ChaCha8Rng",
        "games": args.games,
        "source_records_verified": args.games,
        "source_passes": source_passes,
        "early_terminal_games": terminal_before_max,
        "stone_counts": args.stones,
        "forward_discs": args.discs,
        "comparison_limit": args.comparisons,
        "draw_limit": args.max_draws,
        "groups": groups,
        "groups_by_stones": groups_by_stones,
        "rows": rows,
        "observed_rows": observed_rows,
        "unlabelled_comparison_rows": rows - observed_rows,
        "empty_observed_groups_skipped": empty_observed_groups,
        "no_comparison_groups_skipped": no_comparison_groups,
        "groups_excluded_for_train_overlap": overlap_groups,
        "groups_reaching_draw_cap": draw_cap_groups,
        "sampled_placement_predecessors": sampled_placements,
        "possible_placement_predecessors": possible_placements,
        "examined_candidates": examined_candidates,
        "filtered_candidates": filtered_candidates,
        "canonical_transforms": ["identity", "rotate180", "transpose", "anti_transpose"],
        "canonical_key_count": keys.len(),
        "data_sha256": sha256(&args.output)?,
        "source_records_sha256": sha256(&args.records_out)?,
        "canonical_keys_sha256": sha256(&args.keys_out)?,
        "excluded_keys_sha256": args.exclude_keys.as_ref().map(|path| sha256(path)).transpose()?,
        "elapsed_seconds": started.elapsed().as_secs_f64(),
    });
    fs::write(
        &args.metadata_out,
        format!("{}\n", serde_json::to_string_pretty(&metadata)?),
    )?;
    eprintln!("{}", serde_json::to_string(&metadata)?);
    Ok(())
}
