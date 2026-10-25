//! Reproducible reverse-search comparisons using the same exact goals and table.

use clap::{Parser, ValueEnum};
use othello_complexity_rs::io::{format_position, parse_file_to_positions};
use othello_complexity_rs::othello::{BoardSymmetry, Position};
use othello_complexity_rs::record::{GoalMatch, PositionTransform, ReachabilityRecord};
use othello_complexity_rs::search::beam::{
    retrospective_beam_search, retrospective_layered_beam_search,
    retrospective_learned_beam_search, BeamConfig, BeamHeuristic, BeamStats,
};
use othello_complexity_rs::search::core::{successors, SearchOutcome};
use othello_complexity_rs::search::forward::ForwardTable;
use othello_complexity_rs::search::learned::LearnedModel;
use othello_complexity_rs::search::parallel_gbfs::parallel_retrospective_greedy_best_first_search_with_stats;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::error::Error;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Algorithm {
    Gbfs,
    BeamBranching,
    BeamMobility,
    BeamCoherence,
    BeamFrontier,
    BeamContinuation,
    BestFirstBranching,
    BestFirstMobility,
    BestFirstCoherence,
    BestFirstFrontier,
    BestFirstContinuation,
    LearnedBeam,
    LearnedBestFirst,
}

impl Algorithm {
    fn name(self) -> &'static str {
        match self {
            Self::Gbfs => "gbfs",
            Self::BeamBranching => "beam-branching",
            Self::BeamMobility => "beam-mobility",
            Self::BeamCoherence => "beam-coherence",
            Self::BeamFrontier => "beam-frontier",
            Self::BeamContinuation => "beam-continuation",
            Self::BestFirstBranching => "best-first-branching",
            Self::BestFirstMobility => "best-first-mobility",
            Self::BestFirstCoherence => "best-first-coherence",
            Self::BestFirstFrontier => "best-first-frontier",
            Self::BestFirstContinuation => "best-first-continuation",
            Self::LearnedBeam => "learned-beam",
            Self::LearnedBestFirst => "learned-best-first",
        }
    }

    fn is_learned(self) -> bool {
        matches!(self, Self::LearnedBeam | Self::LearnedBestFirst)
    }
}

#[derive(Parser)]
#[command(about = "Compare reverse searches on exact positions; emit one CSV row per search")]
struct Args {
    /// Existing 66-character positions. Without this, generate legal playouts.
    #[arg(long)]
    input: Option<PathBuf>,
    /// Number of independently generated games at each requested stone count.
    #[arg(long, default_value_t = 4)]
    samples: usize,
    #[arg(long, value_delimiter = ',', default_value = "12,16,20,24", value_parser = clap::value_parser!(u32).range(4..=64))]
    stones: Vec<u32>,
    #[arg(long, default_value_t = 20261025)]
    seed: u64,
    /// Forward table is constructed once, outside every per-search timer.
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(4..=64))]
    discs: u32,
    /// GBFS bounds registered states; beam bounds generated candidates.
    #[arg(long, default_value_t = 100_000)]
    max_nodes: usize,
    /// Override the beam candidate budget while keeping the GBFS budget unchanged.
    #[arg(long)]
    beam_max_nodes: Option<usize>,
    /// Applies to GBFS only. Beam is sequential.
    #[arg(long, default_value_t = 1)]
    threads: usize,
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_value = "gbfs,best-first-branching,best-first-mobility"
    )]
    algorithms: Vec<Algorithm>,
    /// Ranking model; repeat this option to compare several models in one run.
    #[arg(long = "model", value_name = "FILE", action = clap::ArgAction::Append)]
    models: Vec<PathBuf>,
    #[arg(long, value_delimiter = ',', default_value = "32")]
    widths: Vec<usize>,
    #[arg(long, default_value_t = 1)]
    repeats: usize,
    /// CSV output file; omit to write to stdout. Rows are flushed immediately.
    #[arg(long)]
    output: Option<PathBuf>,
    /// Save the exact corpus, including its order, for a later comparison.
    #[arg(long)]
    positions_out: Option<PathBuf>,
    /// Save source certificates for generated goals, before timing searches.
    #[arg(long, conflicts_with = "input")]
    source_records_out: Option<PathBuf>,
    /// Save every Found certificate, including repeats and algorithms, for external verification.
    #[arg(long)]
    records_out: Option<PathBuf>,
    /// Training/validation canonical positions; reject generated goals in these orbits.
    #[arg(long, value_name = "FILE", action = clap::ArgAction::Append)]
    exclude_keys: Vec<PathBuf>,
    /// Record the goal-overlap check, replacements, and exclusion-file hashes.
    #[arg(long)]
    exclusion_report_out: Option<PathBuf>,
}

struct Case {
    position: Position,
    source_record: Option<ReachabilityRecord>,
}

struct Metrics {
    generated: usize,
    expanded: usize,
    retained: Option<usize>,
    peak_frontier: usize,
    pruned: bool,
    model_evaluations: usize,
    model_inference_nanos: u64,
}

impl From<BeamStats> for Metrics {
    fn from(stats: BeamStats) -> Self {
        Self {
            generated: stats.generated,
            expanded: stats.expanded,
            retained: Some(stats.retained),
            peak_frontier: stats.peak_frontier,
            pruned: stats.pruned,
            model_evaluations: stats.model_evaluations,
            model_inference_nanos: stats.model_inference_nanos,
        }
    }
}

fn canonical(position: Position) -> Position {
    [
        BoardSymmetry::Identity,
        BoardSymmetry::Rotate180,
        BoardSymmetry::Transpose,
        BoardSymmetry::AntiTranspose,
    ]
    .into_iter()
    .map(|symmetry| symmetry.apply_position(position))
    .min()
    .unwrap()
}

fn record_exclusions(
    args: &Args,
    keys: &HashSet<Position>,
    cases: usize,
    replacements: usize,
) -> Result<(), Box<dyn Error>> {
    if let Some(path) = &args.exclusion_report_out {
        let inputs = args
            .exclude_keys
            .iter()
            .map(|source| {
                Ok(serde_json::json!({
                    "path": source,
                    "sha256": format!("{:x}", Sha256::digest(std::fs::read(source)?)),
                }))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let mut file = BufWriter::new(File::create(path)?);
        serde_json::to_writer_pretty(
            &mut file,
            &serde_json::json!({
                "schema": "reverse-search-query-exclusion-v1",
                "excluded_key_count": keys.len(),
                "checked_queries": cases,
                "generated_goals_replaced_for_overlap": replacements,
                "canonical_transforms": ["identity", "rotate180", "transpose", "anti_transpose"],
                "excluded_files": inputs,
                "source": args.input,
                "seed": if args.input.is_none() { Some(args.seed) } else { None },
                "remaining_goal_overlap": 0,
            }),
        )?;
        writeln!(file)?;
        file.flush()?;
    }
    Ok(())
}

fn corpus(args: &Args) -> Result<Vec<Case>, Box<dyn Error>> {
    let mut excluded = HashSet::new();
    for path in &args.exclude_keys {
        excluded.extend(
            parse_file_to_positions(&path.to_string_lossy())?
                .into_iter()
                .map(canonical),
        );
    }
    if let Some(path) = &args.input {
        let cases: Vec<_> = parse_file_to_positions(&path.to_string_lossy())?
            .into_iter()
            .map(|position| Case {
                position,
                source_record: None,
            })
            .collect();
        if cases
            .iter()
            .any(|case| excluded.contains(&canonical(case.position)))
        {
            return Err(
                "an input goal overlaps training/validation keys; input queries were not dropped"
                    .into(),
            );
        }
        record_exclusions(args, &excluded, cases.len(), 0)?;
        return Ok(cases);
    }
    let mut cases = Vec::new();
    let mut replaced = 0;
    for &stones in &args.stones {
        // A stone-count group stays reproducible if other groups are added.
        let seed = args.seed ^ u64::from(stones).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        for _ in 0..args.samples {
            let mut accepted = None;
            // Early terminal games may not reach the requested count.
            for _ in 0..10_000 {
                let mut position = Position::initial();
                let mut actions = Vec::new();
                while position.board.popcount() < stones {
                    let next = successors(position);
                    if next.is_empty() {
                        break;
                    }
                    let (child, action) = next[rng.random_range(0..next.len())];
                    position = child;
                    actions.push(action);
                }
                if position.board.popcount() == stones {
                    if excluded.contains(&canonical(position)) {
                        replaced += 1;
                        continue;
                    }
                    let record = ReachabilityRecord {
                        input_index: cases.len() + 1,
                        start: Position::initial(),
                        goal: position,
                        goal_match: GoalMatch::Exact,
                        transform: PositionTransform::IDENTITY,
                        actions,
                    };
                    record.verify().map_err(io::Error::other)?;
                    accepted = Some(Case {
                        position,
                        source_record: Some(record),
                    });
                    break;
                }
            }
            cases.push(accepted.ok_or_else(|| {
                io::Error::other(format!(
                    "failed to generate a legal {stones}-stone position"
                ))
            })?);
        }
    }
    record_exclusions(args, &excluded, cases.len(), replaced)?;
    Ok(cases)
}

fn validate_outcome(case: &Case, outcome: &SearchOutcome) -> io::Result<()> {
    match outcome {
        SearchOutcome::Found { record, .. } => {
            if record.start != Position::initial()
                || record.goal != case.position
                || record.goal_match != GoalMatch::Exact
            {
                return Err(io::Error::other(
                    "search returned a certificate for another query",
                ));
            }
            record.verify().map(|_| ()).map_err(io::Error::other)
        }
        SearchOutcome::NotFound { .. } if case.source_record.is_some() => {
            Err(io::Error::other(format!(
                "legal playout was classified as unreachable: {}",
                format_position(case.position)
            )))
        }
        _ => Ok(()),
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    if args.samples == 0 || args.repeats == 0 || args.widths.contains(&0) {
        return Err("samples, repeats, and beam widths must be positive".into());
    }
    if args.threads == 0 {
        return Err("specify a positive GBFS thread count for reproducible comparisons".into());
    }
    if args
        .algorithms
        .iter()
        .any(|algorithm| algorithm.is_learned())
        && args.models.is_empty()
    {
        return Err("learned algorithms require at least one --model FILE".into());
    }
    // Validate models before writing results, then reuse each exact instance.
    let models = args
        .models
        .iter()
        .map(|path| LearnedModel::load(path))
        .collect::<io::Result<Vec<_>>>()?;
    let mut model_hashes = std::collections::HashSet::new();
    if models
        .iter()
        .any(|model| !model_hashes.insert(model.sha256()))
    {
        return Err("the same model JSON bytes were supplied more than once".into());
    }
    let cases = corpus(&args)?;
    if let Some(path) = &args.positions_out {
        let mut file = BufWriter::new(File::create(path)?);
        for case in &cases {
            writeln!(file, "{}", format_position(case.position))?;
        }
        file.flush()?;
    }
    if let Some(path) = &args.source_records_out {
        let mut file = BufWriter::new(File::create(path)?);
        for case in &cases {
            if let Some(record) = &case.source_record {
                writeln!(file, "{}", record.to_csv())?;
            }
        }
        file.flush()?;
    }
    let mut variants = Vec::new();
    for &algorithm in &args.algorithms {
        let widths = match algorithm {
            Algorithm::Gbfs => vec![0],
            Algorithm::LearnedBestFirst => vec![usize::MAX],
            _ => args.widths.clone(),
        };
        for width in widths {
            if algorithm.is_learned() {
                for model_index in 0..models.len() {
                    variants.push((algorithm, width, Some(model_index)));
                }
            } else {
                variants.push((algorithm, width, None));
            }
        }
    }
    if variants.is_empty() || cases.is_empty() {
        return Err("corpus and algorithm list must not be empty".into());
    }
    let started = Instant::now();
    let max_discs = cases
        .iter()
        .map(|case| case.position.board.popcount())
        .max()
        .unwrap();
    let table = ForwardTable::new(args.discs.min(max_discs), None)?;
    let forward_seconds = started.elapsed().as_secs_f64();
    eprintln!(
        "forward table: {} positions, {} stones, {:.6} s; {} goals, {} variants, {} repeats",
        table.len(),
        table.discs(),
        forward_seconds,
        cases.len(),
        variants.len(),
        args.repeats
    );
    let output: Box<dyn Write> = match &args.output {
        Some(path) => Box::new(BufWriter::new(File::create(path)?)),
        None => Box::new(io::stdout().lock()),
    };
    let mut csv = csv::Writer::from_writer(output);
    let mut records = args
        .records_out
        .as_ref()
        .map(|path| File::create(path).map(BufWriter::new))
        .transpose()?;
    csv.write_record([
        "case",
        "stones",
        "known_reachable",
        "algorithm",
        "width",
        "repeat",
        "threads",
        "max_nodes",
        "budget_counts",
        "result",
        "search_seconds",
        "nodes",
        "generated",
        "expanded",
        "retained",
        "peak_frontier",
        "pruned",
        "forward_discs",
        "forward_positions",
        "forward_seconds",
        "seed",
        "position",
        "model_sha256",
        "model_schema",
        "model_path",
        "model_hidden_size",
        "model_evaluations",
        "model_inference_seconds",
    ])?;
    for repeat in 0..args.repeats {
        for (index, case) in cases.iter().enumerate() {
            // Rotate execution order to reduce a fixed warm-cache/order bias.
            for offset in 0..variants.len() {
                let (algorithm, width, model_index) =
                    variants[(offset + index + repeat) % variants.len()];
                let started = Instant::now();
                let (outcome, stats): (SearchOutcome, Metrics) = match algorithm {
                    Algorithm::Gbfs => {
                        let report = parallel_retrospective_greedy_best_first_search_with_stats(
                            case.position,
                            &table,
                            args.max_nodes,
                            false,
                            args.threads,
                        )?;
                        let stats = Metrics {
                            generated: report.stats.generated,
                            expanded: report.stats.expanded,
                            retained: None,
                            peak_frontier: report.stats.peak_frontier,
                            pruned: false,
                            model_evaluations: 0,
                            model_inference_nanos: 0,
                        };
                        (report.outcome, stats)
                    }
                    Algorithm::LearnedBeam | Algorithm::LearnedBestFirst => {
                        let config = BeamConfig {
                            width,
                            max_nodes: args.beam_max_nodes.unwrap_or(args.max_nodes),
                            heuristic: BeamHeuristic::Mobility,
                        };
                        let report = retrospective_learned_beam_search(
                            case.position,
                            &table,
                            &config,
                            &models[model_index.expect("learned variant has a model")],
                        )?;
                        if matches!(report.outcome, SearchOutcome::NotFound { .. }) {
                            return Err(
                                "learned witness search returned an unreachable verdict".into()
                            );
                        }
                        (report.outcome, report.stats.into())
                    }
                    Algorithm::BeamBranching
                    | Algorithm::BeamMobility
                    | Algorithm::BeamCoherence
                    | Algorithm::BeamFrontier
                    | Algorithm::BeamContinuation
                    | Algorithm::BestFirstBranching
                    | Algorithm::BestFirstMobility
                    | Algorithm::BestFirstCoherence
                    | Algorithm::BestFirstFrontier
                    | Algorithm::BestFirstContinuation => {
                        let config = BeamConfig {
                            width,
                            max_nodes: args.beam_max_nodes.unwrap_or(args.max_nodes),
                            heuristic: match algorithm {
                                Algorithm::BeamBranching | Algorithm::BestFirstBranching => {
                                    BeamHeuristic::Branching
                                }
                                Algorithm::BeamCoherence | Algorithm::BestFirstCoherence => {
                                    BeamHeuristic::Coherence
                                }
                                Algorithm::BeamFrontier | Algorithm::BestFirstFrontier => {
                                    BeamHeuristic::Frontier
                                }
                                Algorithm::BeamContinuation | Algorithm::BestFirstContinuation => {
                                    BeamHeuristic::Continuation
                                }
                                _ => BeamHeuristic::Mobility,
                            },
                        };
                        let report = if matches!(
                            algorithm,
                            Algorithm::BestFirstBranching
                                | Algorithm::BestFirstMobility
                                | Algorithm::BestFirstCoherence
                                | Algorithm::BestFirstFrontier
                                | Algorithm::BestFirstContinuation
                        ) {
                            retrospective_beam_search(case.position, &table, &config)?
                        } else {
                            retrospective_layered_beam_search(case.position, &table, &config)?
                        };
                        (report.outcome, report.stats.into())
                    }
                };
                let seconds = started.elapsed().as_secs_f64();
                validate_outcome(case, &outcome)?;
                if let (Some(file), SearchOutcome::Found { record, .. }) = (&mut records, &outcome)
                {
                    let mut record = record.clone();
                    record.input_index = index + 1;
                    writeln!(file, "{}", record.to_csv())?;
                    file.flush()?;
                }
                let result = match &outcome {
                    SearchOutcome::Found { .. } => "OK",
                    SearchOutcome::NotFound { .. } => "NG",
                    SearchOutcome::Unknown { .. } => "UNKNOWN",
                };
                let is_gbfs = matches!(algorithm, Algorithm::Gbfs);
                csv.write_record([
                    (index + 1).to_string(),
                    case.position.board.popcount().to_string(),
                    case.source_record.is_some().to_string(),
                    algorithm.name().to_owned(),
                    if is_gbfs {
                        String::new()
                    } else {
                        width.to_string()
                    },
                    (repeat + 1).to_string(),
                    if is_gbfs { args.threads } else { 1 }.to_string(),
                    if is_gbfs {
                        args.max_nodes
                    } else {
                        args.beam_max_nodes.unwrap_or(args.max_nodes)
                    }
                    .to_string(),
                    if is_gbfs { "registered" } else { "generated" }.to_owned(),
                    result.to_owned(),
                    format!("{seconds:.9}"),
                    outcome.nodes().to_string(),
                    stats.generated.to_string(),
                    stats.expanded.to_string(),
                    stats.retained.map(|n| n.to_string()).unwrap_or_default(),
                    stats.peak_frontier.to_string(),
                    stats.pruned.to_string(),
                    table.discs().to_string(),
                    table.len().to_string(),
                    format!("{forward_seconds:.9}"),
                    if args.input.is_some() {
                        String::new()
                    } else {
                        args.seed.to_string()
                    },
                    format_position(case.position),
                    model_index
                        .map(|i| models[i].sha256().to_owned())
                        .unwrap_or_default(),
                    model_index
                        .map(|i| models[i].schema().to_owned())
                        .unwrap_or_default(),
                    model_index
                        .map(|i| args.models[i].display().to_string())
                        .unwrap_or_default(),
                    model_index
                        .map(|i| models[i].hidden_size().to_string())
                        .unwrap_or_default(),
                    stats.model_evaluations.to_string(),
                    format!("{:.9}", stats.model_inference_nanos as f64 / 1e9),
                ])?;
                csv.flush()?;
                eprintln!(
                    "case {}/{} {} width={} model={} repeat={} {} nodes={} {:.6}s",
                    index + 1,
                    cases.len(),
                    algorithm.name(),
                    width,
                    model_index
                        .map(|i| args.models[i].display().to_string())
                        .unwrap_or_default(),
                    repeat + 1,
                    result,
                    outcome.nodes(),
                    seconds
                );
            }
        }
    }
    Ok(())
}
