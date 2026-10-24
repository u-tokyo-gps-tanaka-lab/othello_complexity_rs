use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::num::NonZeroU128;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::filters::sat::{run_with_options, RunOptions, SatEncoding};
use crate::filters::{Filter, FilterResult};
use crate::interval::{self, Interval};
use crate::io::{format_position, parse_file_to_positions};
use crate::othello::Position;
use crate::query::GoalClassification;
use crate::record::{GoalMatch, ReachabilityRecord};
use crate::sampling::{create_output_dir, read_sample_dir, Population, Sample};
use crate::search::run::{run_strategy, Strategy};

const REVERSE_STRATEGIES: [&str; 7] = [
    "dfs",
    "dfs-move-ordering",
    "dfs-parallel",
    "gbfs-parallel",
    "bfs-external",
    "bfs-external-parallel",
    "bfs-inmemory-parallel",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Status {
    Ok,
    Ng,
    Unknown,
}

impl Status {
    const fn count_index(self) -> usize {
        match self {
            Self::Ok => 0,
            Self::Ng => 1,
            Self::Unknown => 2,
        }
    }
}

struct FilterOutput {
    filter: Filter,
    ok: Vec<Position>,
    ng: Vec<Position>,
}

#[derive(Clone, Copy, Debug)]
enum StageKind {
    Filters,
    Sat,
    ReverseSearch,
}

struct SummaryLink {
    kind: StageKind,
    from: Option<PathBuf>,
    sample_dir: Option<PathBuf>,
}

struct SavedStage {
    directory: PathBuf,
    kind: StageKind,
}

struct ChainState {
    from_dir: PathBuf,
    sample: Sample,
    classifications: HashMap<Position, Status>,
    pending_path: PathBuf,
    pending: Vec<Position>,
}

/// Run the fixed filter pipeline over a validated sample.
pub fn filters(sample_dir: &Path, out_dir: &Path, alpha: f64) -> io::Result<Interval> {
    require_new_output_dir(out_dir)?;
    interval::validate_alpha(alpha)?;

    let sample_dir = canonical_directory(sample_dir, "sample directory")?;
    let sample = read_sample_dir(&sample_dir)?;
    let unique = deduplicate(&sample.positions);
    let mut classifications = unique
        .iter()
        .copied()
        .map(|position| (position, Status::Unknown))
        .collect::<HashMap<_, _>>();
    let mut outputs: Vec<FilterOutput> = Vec::with_capacity(Filter::PIPELINE.len());

    for filter in Filter::PIPELINE {
        let input = outputs
            .last()
            .map_or(unique.as_slice(), |output| output.ok.as_slice());
        let started = Instant::now();
        let mut ok = Vec::with_capacity(input.len());
        let mut ng = Vec::new();
        let mut undetermined = 0_usize;
        for &position in input {
            let result = filter.check(&position.board).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "{} failed for {}: {error}",
                        filter.name(),
                        format_position(position)
                    ),
                )
            })?;
            match result {
                FilterResult::Pass => ok.push(position),
                FilterResult::Undetermined { .. } => {
                    undetermined = undetermined
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("undetermined count overflow"))?;
                    ok.push(position);
                }
                FilterResult::Rejected { .. } => {
                    set_classification(&mut classifications, position, Status::Ng)?;
                    ng.push(position);
                }
            }
        }
        println!(
            "{} elapsed: {:.3} s, undetermined: {undetermined}",
            filter.name(),
            started.elapsed().as_secs_f64()
        );
        outputs.push(FilterOutput { filter, ok, ng });
    }

    create_output_dir(out_dir)?;
    for output in &outputs {
        let prefix = output.filter.name();
        write_positions(&out_dir.join(format!("{prefix}_OK.txt")), &output.ok)?;
        write_positions(&out_dir.join(format!("{prefix}_NG.txt")), &output.ng)?;
    }
    write_summary(
        out_dir,
        "filters",
        None,
        Some(&sample_dir),
        &sample,
        &classifications,
        alpha,
        None,
    )
}

/// Classify the preceding stage's unresolved positions with a SAT solver.
pub fn sat(
    from: &Path,
    out_dir: &Path,
    alpha: f64,
    encoding: SatEncoding,
    parallel_goals: usize,
    sat_timeout_per_instance: Option<Duration>,
) -> io::Result<Interval> {
    require_new_output_dir(out_dir)?;
    interval::validate_alpha(alpha)?;
    if parallel_goals == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "parallel_goals must be positive",
        ));
    }
    if sat_timeout_per_instance.is_some_and(|timeout| timeout.is_zero()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SAT timeout must be positive",
        ));
    }

    let mut chain = read_chain(from)?;
    let goal_match = population_goal_match(chain.sample.population);
    let outcomes = run_with_options(
        RunOptions {
            start: Position::initial(),
            goals: chain.pending.clone(),
            goal_match,
            encoding,
            parallel_goals,
            show_coords: false,
            show_boards: false,
            verbose: false,
            cnf_dump_dir: None,
            cnf_dump_only: false,
            sat_timeout_per_instance,
        },
        |_| Ok(()),
    )
    .map_err(io::Error::other)?;
    validate_sat_outcomes(&outcomes, &chain.pending, goal_match)?;

    create_output_dir(out_dir)?;
    let prefix = format!("sat_{goal_match}");
    write_sat_outputs(out_dir, &prefix, &chain.pending, &outcomes)?;
    let output_dir = canonical_directory(out_dir, "SAT output directory")?;
    apply_solver_stage(&mut chain, &output_dir, &StageKind::Sat)?;
    write_summary(
        out_dir,
        &format!("sat:{encoding}"),
        Some(&chain.from_dir),
        None,
        &chain.sample,
        &chain.classifications,
        alpha,
        None,
    )
}

/// Classify the preceding stage's unresolved positions with a reverse search.
pub fn reverse_search(
    from: &Path,
    out_dir: &Path,
    alpha: f64,
    strategy: &Strategy,
) -> io::Result<Interval> {
    require_new_output_dir(out_dir)?;
    interval::validate_alpha(alpha)?;

    let mut chain = read_chain(from)?;
    let goal_match = population_goal_match(chain.sample.population);
    create_output_dir(out_dir)?;
    let nodes = run_strategy(strategy, &chain.pending_path, out_dir, goal_match)?;
    if nodes.len() != chain.pending.len() {
        return Err(io::Error::other(format!(
            "reverse search returned {} node counts for {} goals",
            nodes.len(),
            chain.pending.len()
        )));
    }

    let output_dir = canonical_directory(out_dir, "reverse-search output directory")?;
    let stage = StageKind::ReverseSearch;
    apply_solver_stage(&mut chain, &output_dir, &stage)?;
    write_summary(
        out_dir,
        &format!("reverse-search:{}", strategy.name()),
        Some(&chain.from_dir),
        None,
        &chain.sample,
        &chain.classifications,
        alpha,
        Some(&nodes),
    )
}

fn read_chain(from: &Path) -> io::Result<ChainState> {
    let from_dir = canonical_directory(from, "input stage directory")?;
    let mut current = from_dir.clone();
    let mut visited = HashSet::new();
    let mut stages = Vec::new();
    let sample_dir;

    loop {
        if !visited.insert(current.clone()) {
            return Err(invalid(format!(
                "stage chain contains a cycle at {}",
                current.display()
            )));
        }
        let link = read_summary_link(&current)?;
        let is_filters = matches!(link.kind, StageKind::Filters);
        stages.push(SavedStage {
            directory: current.clone(),
            kind: link.kind,
        });
        if is_filters {
            sample_dir = link
                .sample_dir
                .expect("validated filters summary has sample_dir");
            break;
        }
        let parent = link.from.expect("validated solver summary has from");
        current = canonical_directory(&parent, "preceding stage directory")?;
    }

    stages.reverse();
    let sample_dir = canonical_directory(&sample_dir, "sample directory in summary")?;
    let sample = read_sample_dir(&sample_dir)?;
    let unique = deduplicate(&sample.positions);
    let mut classifications = unique
        .iter()
        .copied()
        .map(|position| (position, Status::Unknown))
        .collect::<HashMap<_, _>>();
    let filters_dir = &stages
        .first()
        .expect("a chain ending at filters is nonempty")
        .directory;
    let pending = validate_filter_stage(filters_dir, unique, &mut classifications)?;
    let pending_path = canonical_file(
        &filters_dir.join("lp_OK.txt"),
        "filters unresolved-position file",
    )?;
    let mut state = ChainState {
        from_dir,
        sample,
        classifications,
        pending_path,
        pending,
    };
    for stage in &stages[1..] {
        apply_solver_stage(&mut state, &stage.directory, &stage.kind)?;
    }
    Ok(state)
}

fn read_summary_link(directory: &Path) -> io::Result<SummaryLink> {
    let path = directory.join("summary.json");
    let summary: Value = serde_json::from_reader(File::open(&path)?)
        .map_err(|error| invalid(format!("invalid {}: {error}", path.display())))?;
    let object = summary
        .as_object()
        .ok_or_else(|| invalid(format!("{} must contain a JSON object", path.display())))?;
    let stage_name = object
        .get("stage")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("{} has no string stage", path.display())))?;
    let kind = parse_stage(stage_name)?;

    match kind {
        StageKind::Filters => {
            if object.get("from") != Some(&Value::Null) {
                return Err(invalid("filters summary from must be null"));
            }
            let sample_dir = absolute_summary_path(object.get("sample_dir"), "sample_dir")?;
            Ok(SummaryLink {
                kind: StageKind::Filters,
                from: None,
                sample_dir: Some(sample_dir),
            })
        }
        StageKind::Sat => {
            if object.contains_key("sample_dir") {
                return Err(invalid("solver summary must not contain sample_dir"));
            }
            let from = absolute_summary_path(object.get("from"), "from")?;
            Ok(SummaryLink {
                kind: StageKind::Sat,
                from: Some(from),
                sample_dir: None,
            })
        }
        StageKind::ReverseSearch => {
            if object.contains_key("sample_dir") {
                return Err(invalid("solver summary must not contain sample_dir"));
            }
            let from = absolute_summary_path(object.get("from"), "from")?;
            Ok(SummaryLink {
                kind: StageKind::ReverseSearch,
                from: Some(from),
                sample_dir: None,
            })
        }
    }
}

fn parse_stage(stage: &str) -> io::Result<StageKind> {
    match stage {
        "filters" => Ok(StageKind::Filters),
        _ => {
            if let Some(name) = stage.strip_prefix("sat:") {
                SatEncoding::from_str(name)
                    .map(|_| StageKind::Sat)
                    .map_err(|_| invalid(format!("unknown estimate stage: {stage}")))
            } else if stage
                .strip_prefix("reverse-search:")
                .is_some_and(|name| REVERSE_STRATEGIES.contains(&name))
            {
                Ok(StageKind::ReverseSearch)
            } else {
                Err(invalid(format!("unknown estimate stage: {stage}")))
            }
        }
    }
}

fn absolute_summary_path(value: Option<&Value>, field: &str) -> io::Result<PathBuf> {
    let text = value
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("summary {field} must be an absolute path string")))?;
    let path = PathBuf::from(text);
    if !path.is_absolute() {
        return Err(invalid(format!(
            "summary {field} must be an absolute path string"
        )));
    }
    Ok(path)
}

fn validate_filter_stage(
    directory: &Path,
    mut input: Vec<Position>,
    classifications: &mut HashMap<Position, Status>,
) -> io::Result<Vec<Position>> {
    for filter in Filter::PIPELINE {
        let prefix = filter.name();
        let ok = read_positions(&directory.join(format!("{prefix}_OK.txt")))?;
        let ng = read_positions(&directory.join(format!("{prefix}_NG.txt")))?;
        validate_ordered_partition(&input, &ok, &ng, prefix)?;
        for &position in &ng {
            set_classification(classifications, position, Status::Ng)?;
        }
        input = ok;
    }
    Ok(input)
}

fn validate_ordered_partition(
    input: &[Position],
    ok: &[Position],
    ng: &[Position],
    stage: &str,
) -> io::Result<()> {
    let mut ok_index = 0;
    let mut ng_index = 0;
    for (input_index, expected) in input.iter().enumerate() {
        let next_ok = ok.get(ok_index) == Some(expected);
        let next_ng = ng.get(ng_index) == Some(expected);
        match (next_ok, next_ng) {
            (true, false) => ok_index += 1,
            (false, true) => ng_index += 1,
            (true, true) => {
                return Err(invalid(format!(
                    "{stage} classifies input {} as both OK and NG",
                    input_index + 1
                )))
            }
            (false, false) => {
                return Err(invalid(format!(
                    "{stage} outputs do not preserve input {} ({})",
                    input_index + 1,
                    format_position(*expected)
                )))
            }
        }
    }
    if ok_index != ok.len() || ng_index != ng.len() {
        return Err(invalid(format!(
            "{stage} outputs contain positions outside their input"
        )));
    }
    Ok(())
}

fn apply_solver_stage(
    state: &mut ChainState,
    directory: &Path,
    stage: &StageKind,
) -> io::Result<()> {
    let mode = population_goal_match(state.sample.population);
    let prefix = match stage {
        StageKind::Sat => format!("sat_{mode}"),
        StageKind::ReverseSearch => format!("reverse_{mode}"),
        StageKind::Filters => return Err(invalid("filters cannot follow another stage")),
    };
    let ok = read_positions(&directory.join(format!("{prefix}_OK.txt")))?;
    let ng = read_positions(&directory.join(format!("{prefix}_NG.txt")))?;
    let unknown_path = directory.join(format!("{prefix}_UNKNOWN.txt"));
    let unknown = read_positions(&unknown_path)?;
    let ok_set = unique_positions(&ok, &format!("{prefix} OK output"))?;
    let ng_set = unique_positions(&ng, &format!("{prefix} NG output"))?;
    let unknown_set = unique_positions(&unknown, &format!("{prefix} UNKNOWN output"))?;

    if !ok_set.is_disjoint(&ng_set)
        || !ok_set.is_disjoint(&unknown_set)
        || !ng_set.is_disjoint(&unknown_set)
    {
        return Err(invalid(format!(
            "{prefix} classification outputs are not disjoint"
        )));
    }
    let input_set = unique_positions(&state.pending, "solver input")?;
    let output_count = ok_set
        .len()
        .checked_add(ng_set.len())
        .and_then(|count| count.checked_add(unknown_set.len()))
        .ok_or_else(|| io::Error::other("solver output count overflow"))?;
    if output_count != state.pending.len() {
        return Err(invalid(format!(
            "{prefix} output count differs from its input"
        )));
    }
    let output_set = ok_set
        .iter()
        .chain(&ng_set)
        .chain(&unknown_set)
        .copied()
        .collect::<HashSet<_>>();
    if output_set != input_set {
        return Err(invalid(format!(
            "{prefix} outputs do not partition their input"
        )));
    }

    validate_solver_records(
        &directory.join(format!("{prefix}_records.csv")),
        &ok_set,
        &state.pending,
        mode,
    )?;
    for &position in &ok {
        set_classification(&mut state.classifications, position, Status::Ok)?;
    }
    for &position in &ng {
        set_classification(&mut state.classifications, position, Status::Ng)?;
    }
    state.pending_path = canonical_file(&unknown_path, "solver unresolved-position file")?;
    state.pending = unknown;
    Ok(())
}

fn validate_solver_records(
    path: &Path,
    ok: &HashSet<Position>,
    input: &[Position],
    goal_match: GoalMatch,
) -> io::Result<()> {
    let records = read_records(path)?;
    let mut recorded = HashSet::with_capacity(records.len());
    for record in records {
        validate_record_query(&record, goal_match)?;
        let indexed_goal = record
            .input_index
            .checked_sub(1)
            .and_then(|index| input.get(index))
            .ok_or_else(|| invalid("record input_index is outside the solver input"))?;
        if indexed_goal != &record.goal {
            return Err(invalid("record goal differs from its solver input_index"));
        }
        if !ok.contains(&record.goal) {
            return Err(invalid("record goal is not classified OK"));
        }
        if !recorded.insert(record.goal) {
            return Err(invalid("records file contains duplicate goals"));
        }
    }
    if &recorded != ok {
        return Err(invalid(
            "records file does not contain exactly one record for every OK position",
        ));
    }
    Ok(())
}

fn validate_record_query(record: &ReachabilityRecord, goal_match: GoalMatch) -> io::Result<()> {
    if record.start != Position::initial() {
        return Err(invalid(
            "reachability record does not start at the initial position",
        ));
    }
    if record.goal_match != goal_match {
        return Err(invalid(
            "reachability record mode differs from the sample mode",
        ));
    }
    record
        .verify()
        .map(|_| ())
        .map_err(|error| invalid(format!("reachability record replay failed: {error}")))
}

fn validate_sat_outcomes(
    outcomes: &[GoalClassification],
    input: &[Position],
    goal_match: GoalMatch,
) -> io::Result<()> {
    if outcomes.len() != input.len() {
        return Err(io::Error::other(format!(
            "SAT returned {} classifications for {} goals",
            outcomes.len(),
            input.len()
        )));
    }
    for (index, (outcome, expected_goal)) in outcomes.iter().zip(input).enumerate() {
        if outcome.goal() != *expected_goal
            || outcome.input_index() != index + 1
            || outcome.goal_match() != goal_match
        {
            return Err(io::Error::other(format!(
                "SAT returned a different query for input {}",
                index + 1
            )));
        }
        if let Some(record) = outcome.record() {
            validate_record_query(record, goal_match)?;
        }
    }
    Ok(())
}

fn write_sat_outputs(
    out_dir: &Path,
    prefix: &str,
    input: &[Position],
    outcomes: &[GoalClassification],
) -> io::Result<()> {
    let mut ok = BufWriter::new(File::create_new(out_dir.join(format!("{prefix}_OK.txt")))?);
    let mut ng = BufWriter::new(File::create_new(out_dir.join(format!("{prefix}_NG.txt")))?);
    let mut unknown = BufWriter::new(File::create_new(
        out_dir.join(format!("{prefix}_UNKNOWN.txt")),
    )?);
    let mut records = BufWriter::new(File::create_new(
        out_dir.join(format!("{prefix}_records.csv")),
    )?);
    for (&position, outcome) in input.iter().zip(outcomes) {
        let line = format_position(position);
        match outcome {
            GoalClassification::Reachable { record } => {
                writeln!(ok, "{line}")?;
                writeln!(records, "{}", record.to_csv())?;
            }
            GoalClassification::Unreachable { .. } => writeln!(ng, "{line}")?,
            GoalClassification::Unknown { .. } => writeln!(unknown, "{line}")?,
        }
    }
    ok.flush()?;
    ng.flush()?;
    unknown.flush()?;
    records.flush()
}

#[allow(clippy::too_many_arguments)]
fn write_summary(
    out_dir: &Path,
    stage: &str,
    from: Option<&Path>,
    sample_dir: Option<&Path>,
    sample: &Sample,
    classifications: &HashMap<Position, Status>,
    alpha: f64,
    nodes: Option<&[usize]>,
) -> io::Result<Interval> {
    let counts = count_sample(sample, classifications)?;
    let interval = interval_for_counts(sample, counts, alpha)?;
    let population = sample
        .metadata
        .get("population")
        .cloned()
        .ok_or_else(|| invalid("sampling record has no population field"))?;
    let with_replacement = sample.metadata["with_replacement"]
        .as_bool()
        .ok_or_else(|| invalid("sampling record has no Boolean with_replacement field"))?;
    let mut summary = json!({
        "stage": stage,
        "from": from,
        "population": population,
        "mode": population_goal_match(sample.population).as_str(),
        "with_replacement": with_replacement,
        "accepted_samples": sample.positions.len(),
        "counts": {"OK": counts[0], "NG": counts[1], "UNKNOWN": counts[2]},
        "alpha": alpha,
        "wilson_interval": [interval.lower, interval.upper],
        "reachable_interval": [
            interval.expected_lower.to_string(),
            interval.expected_upper.to_string(),
        ],
    });
    if let Some(sample_dir) = sample_dir {
        summary["sample_dir"] = json!(sample_dir);
    }
    if let Some(nodes) = nodes {
        summary["nodes"] = json!(nodes);
    }
    let mut file = File::create_new(out_dir.join("summary.json"))?;
    serde_json::to_writer_pretty(&mut file, &summary).map_err(io::Error::other)?;
    writeln!(file)?;
    file.sync_all()?;
    Ok(interval)
}

fn count_sample(
    sample: &Sample,
    classifications: &HashMap<Position, Status>,
) -> io::Result<[u64; 3]> {
    let mut counts = [0_u64; 3];
    for position in &sample.positions {
        let status = classifications
            .get(position)
            .copied()
            .ok_or_else(|| io::Error::other("sample position was not classified"))?;
        counts[status.count_index()] = counts[status.count_index()]
            .checked_add(1)
            .ok_or_else(|| io::Error::other("sample count overflow"))?;
    }
    Ok(counts)
}

fn interval_for_counts(sample: &Sample, counts: [u64; 3], alpha: f64) -> io::Result<Interval> {
    let size = sample.population.size(sample.discs).map_err(invalid)?;
    let population_size = NonZeroU128::new(size)
        .ok_or_else(|| io::Error::other("population size must be positive"))?;
    interval::compute(counts[0], counts[1], counts[2], population_size, alpha)
}

fn population_goal_match(population: Population) -> GoalMatch {
    match population {
        Population::Absolute => GoalMatch::Exact,
        Population::Classes => GoalMatch::EquivalenceClass,
    }
}

fn deduplicate(positions: &[Position]) -> Vec<Position> {
    let mut seen = HashSet::with_capacity(positions.len());
    positions
        .iter()
        .copied()
        .filter(|position| seen.insert(*position))
        .collect()
}

fn set_classification(
    classifications: &mut HashMap<Position, Status>,
    position: Position,
    status: Status,
) -> io::Result<()> {
    let current = classifications
        .get_mut(&position)
        .ok_or_else(|| invalid("classification contains a position outside the sample"))?;
    if *current != Status::Unknown {
        return Err(invalid(format!(
            "position was classified more than once: {}",
            format_position(position)
        )));
    }
    *current = status;
    Ok(())
}

fn unique_positions(positions: &[Position], context: &str) -> io::Result<HashSet<Position>> {
    let mut unique = HashSet::with_capacity(positions.len());
    for &position in positions {
        if !unique.insert(position) {
            return Err(invalid(format!(
                "{context} contains a duplicate position: {}",
                format_position(position)
            )));
        }
    }
    Ok(unique)
}

fn read_records(path: &Path) -> io::Result<Vec<ReachabilityRecord>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_path(path)?;
    reader
        .records()
        .enumerate()
        .map(|(index, fields)| {
            let fields = fields
                .map_err(|error| invalid(format!("{}:{}: {error}", path.display(), index + 1)))?;
            ReachabilityRecord::try_from(&fields)
                .map_err(|error| invalid(format!("{}:{}: {error}", path.display(), index + 1)))
        })
        .collect()
}

fn write_positions(path: &Path, positions: &[Position]) -> io::Result<()> {
    let mut file = BufWriter::new(File::create_new(path)?);
    for &position in positions {
        writeln!(file, "{}", format_position(position))?;
    }
    file.flush()
}

fn read_positions(path: &Path) -> io::Result<Vec<Position>> {
    let path_text = path
        .to_str()
        .ok_or_else(|| invalid(format!("path is not valid UTF-8: {}", path.display())))?;
    parse_file_to_positions(path_text)
}

fn canonical_directory(path: &Path, context: &str) -> io::Result<PathBuf> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to resolve {context} {}: {error}", path.display()),
        )
    })?;
    if !fs::metadata(&canonical)?.is_dir() {
        return Err(invalid(format!(
            "{context} is not a directory: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn canonical_file(path: &Path, context: &str) -> io::Result<PathBuf> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to resolve {context} {}: {error}", path.display()),
        )
    })?;
    if !fs::metadata(&canonical)?.is_file() {
        return Err(invalid(format!(
            "{context} is not a file: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn require_new_output_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("output directory already exists: {}", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
