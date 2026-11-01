use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::num::NonZeroU128;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::filters::sat::{
    run_with_options, validate_options, ReverseBfs, RunOptions, SatPortfolio,
};
use crate::filters::{Filter, FilterResult};
use crate::interval::{self, Interval};
use crate::io::{format_position, parse_file_to_positions};
use crate::othello::Position;
use crate::query::GoalClassification;
use crate::record::{GoalMatch, ReachabilityRecord};
use crate::sampling::{self, read_sample_dir, Population, Sample};
use crate::search::run::{run_strategy, Strategy};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SampleSpec {
    pub population: Population,
    pub discs: Option<u32>,
    pub count: usize,
    pub seed: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SatStage {
    pub encodings: SatPortfolio,
    pub seeds: usize,
    pub parallel_goals: usize,
    pub timeout: Option<Duration>,
    pub reverse_bfs: Option<ReverseBfs>,
}

pub struct Pipeline {
    source: Source,
    stages: Vec<Stage>,
}

enum Source {
    Sample(SampleSpec),
    SampleDirectory(PathBuf),
    Run { directory: PathBuf, stages: usize },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    tag = "kind",
    content = "settings",
    rename_all = "kebab-case"
)]
enum Stage {
    Filters,
    Sat(SatStage),
    ReverseSearch(Strategy),
}

impl Stage {
    fn kind(&self) -> &'static str {
        match self {
            Self::Filters => "filters",
            Self::Sat(_) => "sat",
            Self::ReverseSearch(_) => "reverse-search",
        }
    }

    fn directory(&self, index: usize) -> String {
        format!("{:02}-{}", index + 1, self.kind())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Summary {
    sample: SampleRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    branched_from: Option<BranchedFrom>,
    alpha: f64,
    stages: Vec<StageEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BranchedFrom {
    run: PathBuf,
    stages: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SampleRecord {
    source: SampleSource,
    population: PopulationRecord,
    with_replacement: bool,
    rows: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
enum SampleSource {
    Generated(SampleSpec),
    Directory(PathBuf),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PopulationRecord {
    definition: String,
    size: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    discs: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StageEntry {
    stage: Stage,
    directory: String,
    result: Option<StageResult>,
}

impl StageEntry {
    fn new(index: usize, stage: Stage) -> Self {
        Self {
            directory: stage.directory(index),
            stage,
            result: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StageResult {
    input: usize,
    classified: Counts,
    counts: Counts,
    wilson_interval: [f64; 2],
    reachable_interval: [String; 2],
    elapsed_secs: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    nodes: Option<Vec<usize>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Counts {
    #[serde(rename = "OK")]
    ok: u64,
    #[serde(rename = "NG")]
    ng: u64,
    #[serde(rename = "UNKNOWN")]
    unknown: u64,
}

pub struct Report {
    pub ok: u64,
    pub ng: u64,
    pub unknown: u64,
    pub interval: Interval,
}

impl Pipeline {
    pub fn sample(spec: SampleSpec) -> Self {
        Self {
            source: Source::Sample(spec),
            stages: Vec::new(),
        }
    }

    pub fn from_sample_dir(dir: &Path) -> Self {
        Self {
            source: Source::SampleDirectory(dir.to_owned()),
            stages: Vec::new(),
        }
    }

    pub fn from_run(run_dir: &Path, stages: usize) -> Self {
        Self {
            source: Source::Run {
                directory: run_dir.to_owned(),
                stages,
            },
            stages: Vec::new(),
        }
    }

    pub fn filters(mut self) -> Self {
        self.stages.push(Stage::Filters);
        self
    }

    pub fn sat(mut self, stage: SatStage) -> Self {
        self.stages.push(Stage::Sat(stage));
        self
    }

    pub fn reverse_search(mut self, strategy: Strategy) -> Self {
        self.stages.push(Stage::ReverseSearch(strategy));
        self
    }

    pub fn run(self, out_dir: &Path, alpha: f64) -> io::Result<Report> {
        interval::validate_alpha(alpha)?;
        let inherited = match &self.source {
            Source::Run { stages, .. } => *stages,
            Source::Sample(_) | Source::SampleDirectory(_) => 0,
        };
        let total_stages = inherited
            .checked_add(self.stages.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "stage count overflow"))?;
        if total_stages == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "at least one stage is required",
            ));
        }
        validate_output_path(out_dir)?;
        let summary = match read_optional_summary(&out_dir.join("summary.json"))? {
            Some(summary) => summary,
            None => initialize_run(&self.source, &self.stages, out_dir, alpha)?,
        };
        resume_run(&self.source, &self.stages, out_dir, alpha, summary)
    }
}

struct RunState {
    summary: Summary,
    sample: Sample,
    completed: usize,
    table: ClassificationTable,
}

fn initialize_run(
    source: &Source,
    stages: &[Stage],
    out_dir: &Path,
    alpha: f64,
) -> io::Result<Summary> {
    match source {
        Source::Sample(spec) => {
            sampling::validate_generation(spec.population, spec.discs, spec.count)?;
            validate_stages(stages, population_goal_match(spec.population))?;
            sampling::generate(
                &out_dir.join("sample"),
                spec.population,
                spec.discs,
                spec.count,
                spec.seed,
            )?;
            let sample = read_sample_dir(&out_dir.join("sample"))?;
            let summary = Summary {
                sample: sample_record(SampleSource::Generated(spec.clone()), &sample)?,
                branched_from: None,
                alpha,
                stages: stage_entries(stages),
            };
            write_summary_atomic(out_dir, &summary)?;
            Ok(summary)
        }
        Source::SampleDirectory(directory) => {
            let directory = canonical_directory(directory, "sample directory")?;
            reject_same_directory(out_dir, &directory)?;
            let sample = read_sample_dir(&directory)?;
            validate_stages(stages, population_goal_match(sample.population))?;
            let summary = Summary {
                sample: sample_record(SampleSource::Directory(directory), &sample)?,
                branched_from: None,
                alpha,
                stages: stage_entries(stages),
            };
            write_summary_atomic(out_dir, &summary)?;
            Ok(summary)
        }
        Source::Run {
            directory,
            stages: inherited,
        } => initialize_branch(directory, *inherited, stages, out_dir, alpha),
    }
}

fn initialize_branch(
    source_path: &Path,
    inherited: usize,
    tail: &[Stage],
    out_dir: &Path,
    alpha: f64,
) -> io::Result<Summary> {
    let source_dir = canonical_directory(source_path, "source run directory")?;
    reject_same_directory(out_dir, &source_dir)?;
    let source_summary = read_summary(&source_dir.join("summary.json"))?;
    let prefix = inherited_entries(&source_summary, inherited)?.to_vec();
    let sample = read_recorded_sample(&source_dir, &source_summary.sample)?;
    reject_recorded_sample_output(out_dir, &source_dir, &source_summary.sample.source)?;
    let declared = prefix
        .iter()
        .map(|entry| entry.stage.clone())
        .chain(tail.iter().cloned())
        .collect::<Vec<_>>();
    validate_stages(&declared, population_goal_match(sample.population))?;

    fs::create_dir_all(out_dir)?;
    if matches!(&source_summary.sample.source, SampleSource::Generated(_)) {
        copy_directory(&source_dir.join("sample"), &out_dir.join("sample"))?;
    }
    for entry in &prefix {
        copy_directory(
            &source_dir.join(&entry.directory),
            &out_dir.join(&entry.directory),
        )?;
    }
    let summary = Summary {
        sample: source_summary.sample,
        branched_from: Some(BranchedFrom {
            run: source_path.to_owned(),
            stages: inherited,
        }),
        alpha,
        stages: prefix,
    };
    write_summary_atomic(out_dir, &summary)?;
    Ok(summary)
}

fn resume_run(
    source: &Source,
    tail: &[Stage],
    out_dir: &Path,
    alpha: f64,
    mut summary: Summary,
) -> io::Result<Report> {
    let stages = declared_stages(source, tail, out_dir, &summary)?;
    let sample = read_recorded_sample(out_dir, &summary.sample)?;
    reject_recorded_sample_output(out_dir, out_dir, &summary.sample.source)?;
    validate_stages(&stages, population_goal_match(sample.population))?;

    let completed = completed_prefix(&summary.stages)?;
    if stages.len() < completed {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "declared stage list is shorter than the completed prefix",
        ));
    }
    for (index, (saved, declared)) in summary.stages[..completed].iter().zip(&stages).enumerate() {
        validate_stage_directory(index, saved)?;
        if &saved.stage != declared {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("completed stage {} settings differ", index + 1),
            ));
        }
    }

    let mut entries = stages
        .into_iter()
        .enumerate()
        .map(|(index, stage)| {
            if index < completed {
                summary.stages[index].clone()
            } else {
                StageEntry::new(index, stage)
            }
        })
        .collect::<Vec<_>>();
    let mut table = ClassificationTable::new(&sample.positions);
    reload_completed(out_dir, &sample, alpha, completed, &mut entries, &mut table)?;
    summary.alpha = alpha;
    summary.stages = entries;
    let run = RunState {
        summary,
        sample,
        completed,
        table,
    };
    execute_run(out_dir, alpha, run)
}

fn declared_stages(
    source: &Source,
    tail: &[Stage],
    out_dir: &Path,
    summary: &Summary,
) -> io::Result<Vec<Stage>> {
    match source {
        Source::Sample(spec) => {
            sampling::validate_generation(spec.population, spec.discs, spec.count)?;
            if summary.sample.source != SampleSource::Generated(spec.clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "generated sample specification differs from the saved run",
                ));
            }
            if summary.branched_from.is_some() {
                return Err(invalid("unbranched run contains branched_from"));
            }
            Ok(tail.to_vec())
        }
        Source::SampleDirectory(directory) => {
            let directory = canonical_directory(directory, "sample directory")?;
            reject_same_directory(out_dir, &directory)?;
            if summary.sample.source != SampleSource::Directory(directory) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "sample directory differs from the saved run",
                ));
            }
            if summary.branched_from.is_some() {
                return Err(invalid("unbranched run contains branched_from"));
            }
            Ok(tail.to_vec())
        }
        Source::Run {
            directory,
            stages: inherited,
        } => {
            if out_dir == directory {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("output directory is also an input: {}", out_dir.display()),
                ));
            }
            let expected = BranchedFrom {
                run: directory.clone(),
                stages: *inherited,
            };
            if summary.branched_from.as_ref() != Some(&expected) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "branch source differs from the saved run",
                ));
            }
            let prefix = inherited_entries(summary, *inherited)?;
            Ok(prefix
                .iter()
                .map(|entry| entry.stage.clone())
                .chain(tail.iter().cloned())
                .collect())
        }
    }
}

fn inherited_entries(summary: &Summary, count: usize) -> io::Result<&[StageEntry]> {
    let entries = summary
        .stages
        .get(..count)
        .ok_or_else(|| invalid("source run has fewer stages than requested"))?;
    for (index, entry) in entries.iter().enumerate() {
        if entry.result.is_none() {
            return Err(invalid("requested source stages must be completed"));
        }
        validate_stage_directory(index, entry)?;
    }
    Ok(entries)
}

fn completed_prefix(entries: &[StageEntry]) -> io::Result<usize> {
    let completed = entries
        .iter()
        .take_while(|entry| entry.result.is_some())
        .count();
    if entries[completed..]
        .iter()
        .any(|entry| entry.result.is_some())
    {
        return Err(invalid("completed stages must form a contiguous prefix"));
    }
    Ok(completed)
}

fn validate_stage_directory(index: usize, entry: &StageEntry) -> io::Result<()> {
    let expected = entry.stage.directory(index);
    if entry.directory != expected {
        return Err(invalid(format!(
            "stage {} directory must be {expected}",
            index + 1
        )));
    }
    Ok(())
}

fn stage_entries(stages: &[Stage]) -> Vec<StageEntry> {
    stages
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, stage)| StageEntry::new(index, stage))
        .collect()
}

fn validate_stages(stages: &[Stage], mode: GoalMatch) -> io::Result<()> {
    for stage in stages {
        match stage {
            Stage::Filters => {}
            Stage::Sat(stage) => validate_options(&sat_options(stage, mode, Vec::new()))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
            Stage::ReverseSearch(strategy) => strategy.validate()?,
        }
    }
    Ok(())
}

fn execute_run(out_dir: &Path, alpha: f64, mut run: RunState) -> io::Result<Report> {
    write_summary_atomic(out_dir, &run.summary)?;

    for index in run.completed..run.summary.stages.len() {
        let stage = run.summary.stages[index].stage.clone();
        let input = run.table.pending();
        let directory = out_dir.join(&run.summary.stages[index].directory);
        let started = Instant::now();
        let mode = population_goal_match(run.sample.population);
        let nodes = match &stage {
            Stage::Filters => {
                run_filter_stage(&input, &directory)?;
                None
            }
            Stage::Sat(settings) => {
                run_sat_stage(settings, &input, &directory, mode)?;
                None
            }
            Stage::ReverseSearch(strategy) => {
                Some(run_reverse_stage(strategy, &input, &directory, mode)?)
            }
        };
        let loaded = load_stage(&stage, &directory, input, &mut run.table, mode)?;
        run.summary.stages[index].result = Some(stage_result(
            &loaded,
            &run.sample,
            &run.table,
            alpha,
            started.elapsed().as_secs_f64(),
            nodes,
        )?);
        run.completed = index + 1;
        write_summary_atomic(out_dir, &run.summary)?;
    }

    let counts = count_sample(&run.sample, &run.table.statuses)?;
    Ok(Report {
        ok: counts.ok,
        ng: counts.ng,
        unknown: counts.unknown,
        interval: interval_for_counts(&run.sample, counts, alpha)?,
    })
}

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

struct ClassificationTable {
    order: Vec<Position>,
    statuses: HashMap<Position, Status>,
}

impl ClassificationTable {
    fn new(positions: &[Position]) -> Self {
        let order = deduplicate(positions);
        let statuses = order
            .iter()
            .copied()
            .map(|position| (position, Status::Unknown))
            .collect();
        Self { order, statuses }
    }

    fn pending(&self) -> Vec<Position> {
        self.order
            .iter()
            .copied()
            .filter(|position| self.statuses[position] == Status::Unknown)
            .collect()
    }
}

struct FilterOutput {
    filter: Filter,
    ok: Vec<Position>,
    ng: Vec<Position>,
}

fn run_filter_stage(input: &[Position], directory: &Path) -> io::Result<()> {
    let mut current = input.to_vec();
    let mut outputs = Vec::with_capacity(Filter::PIPELINE.len());
    for filter in Filter::PIPELINE {
        let started = Instant::now();
        let results = current
            .par_iter()
            .map(|&position| {
                filter.check(&position.board).map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!(
                            "{} failed for {}: {error}",
                            filter.name(),
                            format_position(position)
                        ),
                    )
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        let mut ok = Vec::with_capacity(current.len());
        let mut ng = Vec::new();
        let mut undetermined = 0_usize;
        for (&position, result) in current.iter().zip(results) {
            match result {
                FilterResult::Pass => ok.push(position),
                FilterResult::Undetermined { .. } => {
                    undetermined = undetermined
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("undetermined count overflow"))?;
                    ok.push(position);
                }
                FilterResult::Rejected { .. } => ng.push(position),
            }
        }
        println!(
            "{} elapsed: {:.3} s, undetermined: {undetermined}",
            filter.name(),
            started.elapsed().as_secs_f64()
        );
        current = ok.clone();
        outputs.push(FilterOutput { filter, ok, ng });
    }

    fs::create_dir_all(directory)?;
    for output in outputs {
        let prefix = output.filter.name();
        write_positions(&directory.join(format!("{prefix}_OK.txt")), &output.ok)?;
        write_positions(&directory.join(format!("{prefix}_NG.txt")), &output.ng)?;
    }
    Ok(())
}

fn sat_options(stage: &SatStage, mode: GoalMatch, goals: Vec<Position>) -> RunOptions {
    RunOptions {
        start: Position::initial(),
        goals,
        goal_match: mode,
        encodings: stage.encodings.clone(),
        seeds: stage.seeds,
        parallel_goals: stage.parallel_goals,
        show_coords: false,
        show_boards: false,
        verbose: false,
        cnf_dump_dir: None,
        cnf_dump_only: false,
        sat_timeout_per_instance: stage.timeout,
        reverse_bfs: stage.reverse_bfs,
    }
}

fn run_sat_stage(
    stage: &SatStage,
    input: &[Position],
    directory: &Path,
    mode: GoalMatch,
) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let prefix = format!("sat_{mode}");
    let mut writers = SatWriters::new(directory, &prefix)?;
    let outcomes = run_with_options(sat_options(stage, mode, input.to_vec()), |outcome| {
        writers.write(outcome).map_err(|error| error.to_string())
    })
    .map_err(io::Error::other)?;
    writers.flush()?;
    validate_sat_outcomes(&outcomes, input, mode, &stage.encodings)
}

struct SatWriters {
    ok: BufWriter<File>,
    ng: BufWriter<File>,
    unknown: BufWriter<File>,
    records: BufWriter<File>,
}

impl SatWriters {
    fn new(directory: &Path, prefix: &str) -> io::Result<Self> {
        Ok(Self {
            ok: BufWriter::new(File::create(directory.join(format!("{prefix}_OK.txt")))?),
            ng: BufWriter::new(File::create(directory.join(format!("{prefix}_NG.txt")))?),
            unknown: BufWriter::new(File::create(
                directory.join(format!("{prefix}_UNKNOWN.txt")),
            )?),
            records: BufWriter::new(File::create(
                directory.join(format!("{prefix}_records.csv")),
            )?),
        })
    }

    fn write(&mut self, outcome: &GoalClassification) -> io::Result<()> {
        let line = format_position(outcome.goal());
        match outcome {
            GoalClassification::Reachable { record } => {
                writeln!(self.ok, "{line}")?;
                writeln!(self.records, "{}", record.to_csv())?;
                self.ok.flush()?;
                self.records.flush()?;
            }
            GoalClassification::Unreachable { .. } => {
                writeln!(self.ng, "{line}")?;
                self.ng.flush()?;
            }
            GoalClassification::Unknown { .. } => {
                writeln!(self.unknown, "{line}")?;
                self.unknown.flush()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.ok.flush()?;
        self.ng.flush()?;
        self.unknown.flush()?;
        self.records.flush()
    }
}

fn run_reverse_stage(
    strategy: &Strategy,
    input: &[Position],
    directory: &Path,
    mode: GoalMatch,
) -> io::Result<Vec<usize>> {
    fs::create_dir_all(directory)?;
    let results = run_strategy(strategy, input, directory, mode)?;
    if results.len() != input.len() {
        return Err(io::Error::other(format!(
            "reverse search returned {} results for {} goals",
            results.len(),
            input.len()
        )));
    }
    let mut nodes = Vec::with_capacity(results.len());
    for (index, ((outcome, count), expected)) in results.iter().zip(input).enumerate() {
        validate_outcome_query(outcome, *expected, index + 1, mode, "reverse search")?;
        nodes.push(*count);
    }
    Ok(nodes)
}

fn validate_sat_outcomes(
    outcomes: &[GoalClassification],
    input: &[Position],
    mode: GoalMatch,
    encodings: &SatPortfolio,
) -> io::Result<()> {
    if outcomes.len() != input.len() {
        return Err(io::Error::other(format!(
            "SAT returned {} classifications for {} goals",
            outcomes.len(),
            input.len()
        )));
    }
    for (index, (outcome, expected)) in outcomes.iter().zip(input).enumerate() {
        if !encodings.proves_unreachable()
            && matches!(outcome, GoalClassification::Unreachable { .. })
        {
            return Err(invalid(
                "witness SAT stage returned an unreachable position",
            ));
        }
        validate_outcome_query(outcome, *expected, index + 1, mode, "SAT")?;
    }
    Ok(())
}

fn validate_outcome_query(
    outcome: &GoalClassification,
    goal: Position,
    input_index: usize,
    mode: GoalMatch,
    context: &str,
) -> io::Result<()> {
    if outcome.goal() != goal
        || outcome.input_index() != input_index
        || outcome.goal_match() != mode
    {
        return Err(io::Error::other(format!(
            "{context} returned a different query for input {input_index}"
        )));
    }
    if let Some(record) = outcome.record() {
        validate_record_query(record, mode)?;
    }
    Ok(())
}

struct StageLoad {
    input: usize,
    classified: [usize; 3],
}

fn load_stage(
    stage: &Stage,
    directory: &Path,
    input: Vec<Position>,
    table: &mut ClassificationTable,
    mode: GoalMatch,
) -> io::Result<StageLoad> {
    match stage {
        Stage::Filters => load_filter_stage(directory, input, table),
        Stage::Sat(settings) => load_solver_stage(directory, input, table, mode, Some(settings)),
        Stage::ReverseSearch(_) => load_solver_stage(directory, input, table, mode, None),
    }
}

fn load_filter_stage(
    directory: &Path,
    mut input: Vec<Position>,
    table: &mut ClassificationTable,
) -> io::Result<StageLoad> {
    let stage_input = input.len();
    for filter in Filter::PIPELINE {
        let prefix = filter.name();
        let ok = read_positions(&directory.join(format!("{prefix}_OK.txt")))?;
        let ng = read_positions(&directory.join(format!("{prefix}_NG.txt")))?;
        validate_ordered_partition(&input, &ok, &ng, prefix)?;
        for position in ng {
            set_classification(&mut table.statuses, position, Status::Ng)?;
        }
        input = ok;
    }
    Ok(StageLoad {
        input: stage_input,
        classified: [0, stage_input - input.len(), input.len()],
    })
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

fn load_solver_stage(
    directory: &Path,
    input: Vec<Position>,
    table: &mut ClassificationTable,
    mode: GoalMatch,
    sat: Option<&SatStage>,
) -> io::Result<StageLoad> {
    let prefix = if sat.is_some() {
        format!("sat_{mode}")
    } else {
        format!("reverse_{mode}")
    };
    let ok = read_positions(&directory.join(format!("{prefix}_OK.txt")))?;
    let ng = read_positions(&directory.join(format!("{prefix}_NG.txt")))?;
    let unknown = read_positions(&directory.join(format!("{prefix}_UNKNOWN.txt")))?;
    if sat.is_some_and(|settings| !settings.encodings.proves_unreachable()) && !ng.is_empty() {
        return Err(invalid("witness SAT stage contains unreachable positions"));
    }

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
    let input_set = unique_positions(&input, "solver input")?;
    let output_set = ok_set
        .iter()
        .chain(&ng_set)
        .chain(&unknown_set)
        .copied()
        .collect::<HashSet<_>>();
    if output_set != input_set || ok_set.len() + ng_set.len() + unknown_set.len() != input.len() {
        return Err(invalid(format!(
            "{prefix} outputs do not partition their input"
        )));
    }

    validate_solver_records(
        &directory.join(format!("{prefix}_records.csv")),
        &ok_set,
        &input,
        mode,
    )?;
    for position in &ok {
        set_classification(&mut table.statuses, *position, Status::Ok)?;
    }
    for position in &ng {
        set_classification(&mut table.statuses, *position, Status::Ng)?;
    }
    Ok(StageLoad {
        input: input.len(),
        classified: [ok.len(), ng.len(), unknown.len()],
    })
}

fn validate_solver_records(
    path: &Path,
    ok: &HashSet<Position>,
    input: &[Position],
    mode: GoalMatch,
) -> io::Result<()> {
    let records = read_records(path)?;
    let mut recorded = HashSet::with_capacity(records.len());
    for record in records {
        validate_record_query(&record, mode)?;
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

fn validate_record_query(record: &ReachabilityRecord, mode: GoalMatch) -> io::Result<()> {
    if record.start != Position::initial() {
        return Err(invalid(
            "reachability record does not start at the initial position",
        ));
    }
    if record.goal_match != mode {
        return Err(invalid(
            "reachability record mode differs from the sample mode",
        ));
    }
    record
        .verify()
        .map(|_| ())
        .map_err(|error| invalid(format!("reachability record replay failed: {error}")))
}

fn reload_completed(
    out_dir: &Path,
    sample: &Sample,
    alpha: f64,
    completed: usize,
    entries: &mut [StageEntry],
    table: &mut ClassificationTable,
) -> io::Result<()> {
    let mode = population_goal_match(sample.population);
    for entry in entries.iter_mut().take(completed) {
        let saved = entry
            .result
            .as_ref()
            .ok_or_else(|| invalid("completed stage has no result"))?;
        let elapsed = saved.elapsed_secs;
        let nodes = match &entry.stage {
            Stage::ReverseSearch(_) => Some(
                saved
                    .nodes
                    .clone()
                    .ok_or_else(|| invalid("completed reverse-search stage has no nodes array"))?,
            ),
            Stage::Filters | Stage::Sat(_) => {
                if saved.nodes.is_some() {
                    return Err(invalid("only reverse-search stages may contain nodes"));
                }
                None
            }
        };
        let input = table.pending();
        let directory = out_dir.join(&entry.directory);
        let loaded = load_stage(&entry.stage, &directory, input, table, mode)?;
        if let Some(nodes) = &nodes {
            if nodes.len() != loaded.input {
                return Err(invalid(
                    "completed reverse-search nodes length differs from its input",
                ));
            }
        }
        entry.result = Some(stage_result(&loaded, sample, table, alpha, elapsed, nodes)?);
    }
    Ok(())
}

fn stage_result(
    loaded: &StageLoad,
    sample: &Sample,
    table: &ClassificationTable,
    alpha: f64,
    elapsed: f64,
    nodes: Option<Vec<usize>>,
) -> io::Result<StageResult> {
    let counts = count_sample(sample, &table.statuses)?;
    let interval = interval_for_counts(sample, counts, alpha)?;
    Ok(StageResult {
        input: loaded.input,
        classified: counts_from_usizes(loaded.classified)?,
        counts,
        wilson_interval: [interval.lower, interval.upper],
        reachable_interval: [
            interval.expected_lower.to_string(),
            interval.expected_upper.to_string(),
        ],
        elapsed_secs: elapsed,
        nodes,
    })
}

fn counts_from_usizes(counts: [usize; 3]) -> io::Result<Counts> {
    Ok(Counts {
        ok: u64::try_from(counts[0]).map_err(|_| io::Error::other("OK count overflow"))?,
        ng: u64::try_from(counts[1]).map_err(|_| io::Error::other("NG count overflow"))?,
        unknown: u64::try_from(counts[2])
            .map_err(|_| io::Error::other("UNKNOWN count overflow"))?,
    })
}

fn sample_record(source: SampleSource, sample: &Sample) -> io::Result<SampleRecord> {
    let population = sample
        .metadata
        .get("population")
        .cloned()
        .ok_or_else(|| invalid("sampling record has no population field"))?;
    let population = serde_json::from_value(population)
        .map_err(|error| invalid(format!("invalid sampling population: {error}")))?;
    let with_replacement = sample.metadata["with_replacement"]
        .as_bool()
        .ok_or_else(|| invalid("sampling record has no Boolean with_replacement field"))?;
    Ok(SampleRecord {
        source,
        population,
        with_replacement,
        rows: sample.positions.len(),
    })
}

fn read_recorded_sample(base: &Path, record: &SampleRecord) -> io::Result<Sample> {
    let directory = match &record.source {
        SampleSource::Generated(_) => {
            canonical_directory(&base.join("sample"), "generated sample directory")?
        }
        SampleSource::Directory(directory) => {
            let canonical = canonical_directory(directory, "sample directory in summary")?;
            if &canonical != directory {
                return Err(invalid("saved sample directory is not canonical"));
            }
            canonical
        }
    };
    let sample = read_sample_dir(&directory)?;
    let actual_source = match &record.source {
        SampleSource::Generated(_) => SampleSource::Generated(SampleSpec {
            population: sample.population,
            discs: sample.discs,
            count: sample.positions.len(),
            seed: sample.metadata["seed"]
                .as_u64()
                .ok_or_else(|| invalid("sampling record has no u64 seed"))?,
        }),
        SampleSource::Directory(_) => SampleSource::Directory(directory),
    };
    if &sample_record(actual_source, &sample)? != record {
        return Err(invalid(
            "saved sample summary differs from its sample files",
        ));
    }
    Ok(sample)
}

fn reject_recorded_sample_output(
    output: &Path,
    sample_base: &Path,
    source: &SampleSource,
) -> io::Result<()> {
    let directory = match source {
        SampleSource::Generated(_) => {
            canonical_directory(&sample_base.join("sample"), "sample directory")?
        }
        SampleSource::Directory(directory) => directory.clone(),
    };
    reject_same_directory(output, &directory)
}

fn write_summary_atomic(directory: &Path, summary: &Summary) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let temporary = directory.join("summary.json.tmp");
    let mut file = File::create(&temporary)?;
    serde_json::to_writer_pretty(&mut file, summary).map_err(io::Error::other)?;
    writeln!(file)?;
    file.sync_all()?;
    fs::rename(&temporary, directory.join("summary.json"))?;
    File::open(directory)?.sync_all()
}

fn read_optional_summary(path: &Path) -> io::Result<Option<Summary>> {
    match File::open(path) {
        Ok(file) => serde_json::from_reader(file)
            .map(Some)
            .map_err(|error| invalid(format!("invalid {}: {error}", path.display()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_summary(path: &Path) -> io::Result<Summary> {
    read_optional_summary(path)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("source run has no {}", path.display()),
        )
    })
}

fn copy_directory(source: &Path, destination: &Path) -> io::Result<()> {
    if !fs::metadata(source)?.is_dir() {
        return Err(invalid(format!(
            "copy source is not a directory: {}",
            source.display()
        )));
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(invalid(format!(
                "cannot copy non-file entry: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn validate_output_path(path: &Path) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("output path is not a directory: {}", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn reject_same_directory(output: &Path, input: &Path) -> io::Result<()> {
    match fs::canonicalize(output) {
        Ok(output) if output == input => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("output directory is also an input: {}", output.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn canonical_directory(path: &Path, context: &str) -> io::Result<PathBuf> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to resolve {context} {}: {error}", path.display()),
        )
    })?;
    if !fs::metadata(&canonical)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{context} is not a directory: {}", canonical.display()),
        ));
    }
    Ok(canonical)
}

fn count_sample(
    sample: &Sample,
    classifications: &HashMap<Position, Status>,
) -> io::Result<Counts> {
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
    Ok(Counts {
        ok: counts[0],
        ng: counts[1],
        unknown: counts[2],
    })
}

fn interval_for_counts(sample: &Sample, counts: Counts, alpha: f64) -> io::Result<Interval> {
    let size = sample.population.size(sample.discs).map_err(invalid)?;
    let population_size = NonZeroU128::new(size)
        .ok_or_else(|| io::Error::other("population size must be positive"))?;
    interval::compute(counts.ok, counts.ng, counts.unknown, population_size, alpha)
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
    for position in positions {
        if !unique.insert(*position) {
            return Err(invalid(format!(
                "{context} contains a duplicate position: {}",
                format_position(*position)
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
    let mut file = BufWriter::new(File::create(path)?);
    for position in positions {
        writeln!(file, "{}", format_position(*position))?;
    }
    file.flush()
}

fn read_positions(path: &Path) -> io::Result<Vec<Position>> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid(format!("path is not valid UTF-8: {}", path.display())))?;
    parse_file_to_positions(text)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
