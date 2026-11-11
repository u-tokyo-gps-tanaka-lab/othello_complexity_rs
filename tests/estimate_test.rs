//! Integration tests for the in-process estimation pipeline.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::num::NonZeroU128;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use othello_complexity_rs::estimate::{Pipeline, SampleSpec, SatStage};
use othello_complexity_rs::filters::sat::SatEncoding;
use othello_complexity_rs::interval;
use othello_complexity_rs::io::{format_position, parse_position};
use othello_complexity_rs::othello::{Board, Color, Position};
use othello_complexity_rs::record::{GoalMatch, PlyAction, PositionTransform, ReachabilityRecord};
use othello_complexity_rs::sampling::{
    class_representative, convert_classes, generate, read_sample_dir, Population,
};
use othello_complexity_rs::search::{engines::external_bfs::ExternalBfs, Strategy};
use serde_json::{json, Value};

// P(|Z| > 1). With z² = 1, an all-OK sample of size n has bounds
// [n / (n + 1), 1].
const ALPHA: f64 = 0.31731050786291415;
const ABSOLUTE_SIZE: u128 = 32 * 3_u128.pow(60);
const CLASS_SIZE: u128 = 16 * 3_u128.pow(60);
static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "othello-estimate-pipeline-test-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn sample(&self, name: &str, positions: &[Position], population: Value) -> PathBuf {
        self.sample_with_size(name, positions, positions.len(), population)
    }

    fn sample_with_size(
        &self,
        name: &str,
        positions: &[Position],
        sample_size: usize,
        population: Value,
    ) -> PathBuf {
        let directory = self.path(name);
        fs::create_dir(&directory).unwrap();
        write_positions(&directory.join("positions.txt"), positions);
        write_json(
            &directory.join("sampling.json"),
            &json!({
                "schema_version": 1,
                "method": if population.get("discs").is_some() {
                    "fixed-discs"
                } else {
                    "uniform"
                },
                "population": population,
                "uniform": true,
                "with_replacement": true,
                "seed": 1,
                "input_count": sample_size,
                "accepted_count": positions.len(),
            }),
        );
        directory
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn initial() -> Position {
    Position {
        board: Board::new((1 << 28) | (1 << 35), (1 << 27) | (1 << 36)),
        side_to_move: Color::Black,
    }
}

fn after_d3() -> Position {
    Position {
        board: Board::new(1 << 36, (1 << 19) | (1 << 27) | (1 << 28) | (1 << 35)),
        side_to_move: Color::White,
    }
}

fn class_d3() -> Position {
    Position {
        side_to_move: Color::Black,
        ..after_d3()
    }
}

fn disconnected() -> Position {
    let mut position = initial();
    position.board.player |= 1;
    position
}

fn absolute_population() -> Value {
    json!({
        "definition": "absolute_positions_center_occupied",
        "size": ABSOLUTE_SIZE.to_string()
    })
}

fn class_population() -> Value {
    json!({
        "definition": "black_to_move_positions_center_occupied",
        "size": CLASS_SIZE.to_string()
    })
}

fn fixed_absolute(discs: u32, size: &str) -> Value {
    json!({
        "definition": "absolute_positions_fixed_discs",
        "discs": discs,
        "size": size,
    })
}

fn fixed_classes(discs: u32, size: &str) -> Value {
    json!({
        "definition": "black_to_move_positions_fixed_discs",
        "discs": discs,
        "size": size,
    })
}

fn sat_stage(encoding: SatEncoding) -> SatStage {
    SatStage {
        encodings: encoding.into(),
        seeds: 1,
        parallel_goals: 1,
        timeout: None,
    }
}

fn limited_dfs() -> Strategy {
    Strategy::Dfs {
        discs: 4,
        max_nodes: 0,
    }
}

fn complete_dfs() -> Strategy {
    Strategy::Dfs {
        discs: 4,
        max_nodes: 128,
    }
}

fn external(tmp_dir: PathBuf, resume: bool) -> Strategy {
    Strategy::BfsExternal(ExternalBfs {
        discs: 4,
        jobs: 2,
        verbose: 0,
        block_size: 4,
        tmp_dir,
        resume,
    })
}

fn initial_record(input_index: usize) -> ReachabilityRecord {
    ReachabilityRecord {
        input_index,
        start: initial(),
        goal: initial(),
        goal_match: GoalMatch::Exact,
        transform: PositionTransform::IDENTITY,
        actions: Vec::new(),
    }
}

fn d3_record(input_index: usize) -> ReachabilityRecord {
    ReachabilityRecord {
        goal: after_d3(),
        actions: vec![PlyAction::Play(19)],
        ..initial_record(input_index)
    }
}

fn write_positions(path: &Path, positions: &[Position]) {
    fs::write(path, position_text(positions)).unwrap();
}

fn position_text(positions: &[Position]) -> String {
    positions
        .iter()
        .map(|position| format!("{}\n", format_position(*position)))
        .collect()
}

fn read_positions(path: &Path) -> Vec<Position> {
    let text = path.to_str().unwrap();
    othello_complexity_rs::io::parse_file_to_positions(text).unwrap()
}

fn read_records(path: &Path) -> Vec<ReachabilityRecord> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_path(path)
        .unwrap();
    reader
        .records()
        .map(|fields| ReachabilityRecord::try_from(&fields.unwrap()).unwrap())
        .collect()
}

fn write_records(path: &Path, records: &[ReachabilityRecord]) {
    let text: String = records
        .iter()
        .map(|record| format!("{}\n", record.to_csv()))
        .collect();
    fs::write(path, text).unwrap();
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}

fn assert_interval_eq(actual: &interval::Interval, expected: &interval::Interval) {
    assert_eq!(actual.population_size, expected.population_size);
    assert_eq!(actual.sample_size, expected.sample_size);
    close(actual.observed_lower, expected.observed_lower);
    close(actual.observed_upper, expected.observed_upper);
    close(actual.lower, expected.lower);
    close(actual.upper, expected.upper);
    assert_eq!(&actual.expected_lower, &expected.expected_lower);
    assert_eq!(&actual.expected_upper, &expected.expected_upper);
}

fn tree_files(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn collect(root: &Path, directory: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                collect(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                ));
            }
        }
    }

    if !directory.exists() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect(directory, directory, &mut files);
    files
}

fn directory_entries(directory: &Path) -> HashSet<String> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

fn assert_error_without_changes<T>(directory: &Path, operation: impl FnOnce() -> io::Result<T>) {
    let before = tree_files(directory);
    assert!(operation().is_err());
    assert_eq!(tree_files(directory), before);
}

fn copy_tree(source: &Path, destination: &Path) {
    for (relative, bytes) in tree_files(source) {
        let target = destination.join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
}

fn ng_outputs(run: &Path) -> HashSet<Position> {
    tree_files(run)
        .into_iter()
        .filter(|(path, _)| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_NG.txt"))
        })
        .flat_map(|(_, bytes)| {
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|line| parse_position(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn assert_equivalence_ng_runtime_error(sample: &Path, source: &Path, output: &Path) {
    assert!(Pipeline::from_sample_dir(sample)
        .equivalence_ng(source)
        .run(output, ALPHA)
        .is_err());
    let saved = summary(output);
    assert!(saved["stages"][0]["result"].is_null());
    assert!(tree_files(&output.join("01-equivalence-ng")).is_empty());
}

fn summary(run: &Path) -> Value {
    read_json(&run.join("summary.json"))
}

fn small_strategies(fixture: &Fixture, stem: &str) -> Vec<Strategy> {
    let sequential_tmp = fixture.path(&format!("{stem}-bfs-tmp"));
    let parallel_tmp = fixture.path(&format!("{stem}-external-parallel-tmp"));
    fs::create_dir(&sequential_tmp).unwrap();
    fs::create_dir(&parallel_tmp).unwrap();
    vec![
        Strategy::Dfs {
            discs: 4,
            max_nodes: 128,
        },
        Strategy::DfsMoveOrdering {
            discs: 4,
            max_nodes: 128,
        },
        Strategy::DfsParallel {
            discs: 4,
            max_nodes: 128,
            table_size: 128,
            threads: 2,
        },
        Strategy::GbfsParallel {
            discs: 4,
            max_nodes: 128,
            use_lp: false,
            threads: 2,
            parallel_goals: 1,
        },
        external(sequential_tmp, false),
        Strategy::BfsExternalParallel(ExternalBfs {
            discs: 4,
            jobs: 2,
            verbose: 0,
            block_size: 4,
            tmp_dir: parallel_tmp,
            resume: false,
        }),
        Strategy::Bfs {
            discs: 4,
            max_nodes: 128,
            use_lp: false,
            threads: 2,
            parallel_goals: 1,
        },
    ]
}

#[test]
fn filters_deduplicate_in_first_appearance_order_and_keep_row_weights() {
    let fixture = Fixture::new();
    let sample = fixture.sample(
        "sample",
        &[
            initial(),
            disconnected(),
            after_d3(),
            initial(),
            disconnected(),
        ],
        absolute_population(),
    );
    let run = fixture.path("run");
    let report = Pipeline::from_sample_dir(&sample)
        .filters()
        .run(&run, ALPHA)
        .unwrap();

    assert_eq!(
        fs::read_to_string(run.join("01-filters/con_OK.txt")).unwrap(),
        position_text(&[initial(), after_d3()])
    );
    assert_eq!(
        fs::read_to_string(run.join("01-filters/con_NG.txt")).unwrap(),
        position_text(&[disconnected()])
    );
    for filter in ["occupancy", "never_flipped", "lp"] {
        assert_eq!(
            fs::read_to_string(run.join(format!("01-filters/{filter}_OK.txt"))).unwrap(),
            position_text(&[initial(), after_d3()])
        );
        assert!(fs::read(run.join(format!("01-filters/{filter}_NG.txt")))
            .unwrap()
            .is_empty());
    }
    assert_eq!((report.ok, report.ng, report.unknown), (0, 2, 3));
    let saved = summary(&run);
    assert_eq!(saved["stages"][0]["result"]["input"], 3);
    assert_eq!(
        saved["stages"][0]["result"]["classified"],
        json!({"OK": 0, "NG": 1, "UNKNOWN": 2})
    );
    assert_eq!(
        saved["stages"][0]["result"]["counts"],
        json!({"OK": 0, "NG": 2, "UNKNOWN": 3})
    );
}

#[test]
fn fixed_populations_preserve_duplicate_weights_and_goal_matching() {
    for (case, position, population, mode, size, goal_match) in [
        (
            "four-disc absolute positions",
            initial(),
            fixed_absolute(4, "32"),
            "exact",
            32.0,
            GoalMatch::Exact,
        ),
        (
            "four-disc equivalence representatives",
            initial(),
            fixed_classes(4, "16"),
            "equivalence",
            16.0,
            GoalMatch::EquivalenceClass,
        ),
        (
            "five-disc equivalence representatives",
            class_d3(),
            fixed_classes(5, "1920"),
            "equivalence",
            1_920.0,
            GoalMatch::EquivalenceClass,
        ),
    ] {
        let fixture = Fixture::new();
        let sample = fixture.sample("sample", &[position; 3], population);
        let run = fixture.path("run");
        let report = Pipeline::from_sample_dir(&sample)
            .sat(sat_stage(SatEncoding::Ply))
            .run(&run, ALPHA)
            .unwrap();
        assert_eq!((report.ok, report.ng, report.unknown), (3, 0, 0), "{case}");
        let close_case = |actual: f64, expected: f64| {
            assert!(
                (actual - expected).abs() < 1e-12,
                "{case}: {actual} != {expected}"
            );
        };
        close_case(report.interval.lower, 0.75);
        close_case(report.interval.upper, 1.0);
        close_case(
            report.interval.expected_lower.to_string().parse().unwrap(),
            0.75 * size,
        );
        assert_eq!(
            read_positions(&run.join(format!("01-sat/sat_{mode}_OK.txt"))),
            [position],
            "{case}"
        );
        let records = read_records(&run.join(format!("01-sat/sat_{mode}_records.csv")));
        assert_eq!(records.len(), 1, "{case}");
        assert_eq!(records[0].input_index, 1, "{case}");
        assert_eq!(records[0].goal, position, "{case}");
        assert_eq!(records[0].goal_match, goal_match, "{case}");
        records[0]
            .verify()
            .unwrap_or_else(|error| panic!("{case}: {error}"));
    }

    let fixture = Fixture::new();
    let sample = fixture.sample(
        "exact-rejection-sample",
        &[class_d3()],
        fixed_absolute(5, "3840"),
    );
    let run = fixture.path("exact-rejection-run");
    Pipeline::from_sample_dir(&sample)
        .sat(sat_stage(SatEncoding::Ply))
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(
        fs::read_to_string(run.join("01-sat/sat_exact_NG.txt")).unwrap(),
        position_text(&[class_d3()])
    );
    assert!(read_records(&run.join("01-sat/sat_exact_records.csv")).is_empty());
}

#[test]
fn equivalence_interval_reproduces_the_published_sample() {
    let fixture = Fixture::new();
    let positions = vec![initial(); 97];
    let sample = fixture.sample_with_size("sample", &positions, 1_000_000, class_population());
    let run = fixture.path("run");
    let report = Pipeline::from_sample_dir(&sample)
        .sat(sat_stage(SatEncoding::Ply))
        .run(&run, 0.005)
        .unwrap();
    let expected =
        interval::compute(97, 999_903, 0, NonZeroU128::new(CLASS_SIZE).unwrap(), 0.005).unwrap();

    assert_interval_eq(&report.interval, &expected);
    assert_eq!((report.ok, report.ng, report.unknown), (97, 0, 0));
    let lower: f64 = report.interval.expected_lower.to_string().parse().unwrap();
    let upper: f64 = report.interval.expected_upper.to_string().parse().unwrap();
    close((lower / 1e25 * 100.0).round() / 100.0, 4.95);
    close((upper / 1e25 * 100.0).round() / 100.0, 8.74);

    let saved = summary(&run);
    assert_eq!(saved["sample"]["rows"], 97);
    assert_eq!(saved["sample"]["sample_size"], 1_000_000);
    assert_eq!(
        saved["stages"][0]["result"]["counts"],
        json!({"OK": 97, "NG": 0, "UNKNOWN": 0})
    );
}

#[test]
fn equivalence_interval_uses_sample_size_with_all_classifications_present() {
    let fixture = Fixture::new();
    let unreachable = class_representative(disconnected());
    let sample = fixture.sample_with_size(
        "sample",
        &[initial(), unreachable, class_d3()],
        8,
        class_population(),
    );
    let run = fixture.path("run");
    let report = Pipeline::from_sample_dir(&sample)
        .filters()
        .reverse_search(limited_dfs())
        .run(&run, ALPHA)
        .unwrap();
    let expected =
        interval::compute(1, 6, 1, NonZeroU128::new(CLASS_SIZE).unwrap(), ALPHA).unwrap();

    assert_eq!((report.ok, report.ng, report.unknown), (1, 1, 1));
    assert_interval_eq(&report.interval, &expected);
    assert_eq!(
        summary(&run)["stages"][1]["result"]["counts"],
        json!({"OK": 1, "NG": 1, "UNKNOWN": 1})
    );
}

#[test]
fn empty_equivalence_rows_still_produce_an_interval() {
    let fixture = Fixture::new();
    let sample = fixture.sample_with_size("sample", &[], 8, class_population());
    let run = fixture.path("run");
    let report = Pipeline::from_sample_dir(&sample)
        .filters()
        .run(&run, ALPHA)
        .unwrap();
    let expected =
        interval::compute(0, 8, 0, NonZeroU128::new(CLASS_SIZE).unwrap(), ALPHA).unwrap();

    assert_eq!((report.ok, report.ng, report.unknown), (0, 0, 0));
    assert_interval_eq(&report.interval, &expected);
    let saved = summary(&run);
    assert_eq!(saved["sample"]["rows"], 0);
    assert_eq!(saved["sample"]["sample_size"], 8);
}

#[test]
fn filter_ng_skips_sat_and_sat_indices_use_stage_input_order() {
    let fixture = Fixture::new();
    let sample = fixture.sample(
        "sample",
        &[disconnected(), disconnected(), after_d3(), after_d3()],
        fixed_absolute(5, "3840"),
    );
    let run = fixture.path("run");
    let report = Pipeline::from_sample_dir(&sample)
        .filters()
        .sat(sat_stage(SatEncoding::Ply))
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(
        read_positions(&run.join("01-filters/con_NG.txt")),
        [disconnected()]
    );
    assert_eq!(
        read_positions(&run.join("02-sat/sat_exact_OK.txt")),
        [after_d3()]
    );
    assert_eq!(
        read_records(&run.join("02-sat/sat_exact_records.csv")),
        [d3_record(1)]
    );
    assert_eq!((report.ok, report.ng, report.unknown), (2, 2, 0));
    assert_eq!(summary(&run)["stages"][1]["result"]["input"], 1);
}

#[test]
fn stage_kinds_run_in_every_required_order() {
    let fixture = Fixture::new();

    let sample = fixture.sample(
        "sat-reverse-sample",
        &[initial(), after_d3()],
        absolute_population(),
    );
    let run = fixture.path("sat-reverse");
    Pipeline::from_sample_dir(&sample)
        .sat(sat_stage(SatEncoding::Ply))
        .reverse_search(complete_dfs())
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(summary(&run)["stages"][1]["result"]["input"], 0);
    assert!(read_positions(&run.join("02-reverse-search/reverse_exact_OK.txt")).is_empty());

    let sample = fixture.sample(
        "reverse-sat-sample",
        &[initial(), after_d3()],
        absolute_population(),
    );
    let run = fixture.path("reverse-sat");
    Pipeline::from_sample_dir(&sample)
        .reverse_search(limited_dfs())
        .sat(sat_stage(SatEncoding::Ply))
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(summary(&run)["stages"][1]["result"]["input"], 1);
    assert_eq!(
        read_positions(&run.join("02-sat/sat_exact_OK.txt")),
        [after_d3()]
    );

    let sample = fixture.sample(
        "sat-filters-sample",
        &[initial(), disconnected(), class_d3()],
        absolute_population(),
    );
    let run = fixture.path("sat-filters");
    Pipeline::from_sample_dir(&sample)
        .sat(sat_stage(SatEncoding::NoPassWitness))
        .filters()
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(summary(&run)["stages"][1]["result"]["input"], 2);
    assert_eq!(
        read_positions(&run.join("02-filters/con_NG.txt")),
        [disconnected()]
    );
    assert_eq!(
        read_positions(&run.join("02-filters/con_OK.txt")),
        [class_d3()]
    );
}

#[test]
fn nopass_witness_only_settles_ok_and_leaves_ng_for_general_sat() {
    let fixture = Fixture::new();
    let sample = fixture.sample(
        "sample",
        &[initial(), initial(), after_d3(), class_d3(), class_d3()],
        absolute_population(),
    );
    let run = fixture.path("run");
    let report = Pipeline::from_sample_dir(&sample)
        .sat(sat_stage(SatEncoding::NoPassWitness))
        .sat(sat_stage(SatEncoding::PlacementColumns))
        .run(&run, ALPHA)
        .unwrap();
    assert!(read_positions(&run.join("01-sat/sat_exact_NG.txt")).is_empty());
    assert_eq!(
        read_positions(&run.join("01-sat/sat_exact_OK.txt")),
        [initial(), after_d3()]
    );
    assert_eq!(
        read_positions(&run.join("01-sat/sat_exact_UNKNOWN.txt")),
        [class_d3()]
    );
    assert_eq!(
        read_records(&run.join("01-sat/sat_exact_records.csv")),
        [initial_record(1), d3_record(2)]
    );
    assert_eq!(
        read_positions(&run.join("02-sat/sat_exact_NG.txt")),
        [class_d3()]
    );
    assert_eq!((report.ok, report.ng, report.unknown), (3, 2, 0));
}

#[test]
fn settings_validation_is_complete_and_writes_nothing() {
    let fixture = Fixture::new();
    let sample = fixture.sample("sample", &[initial()], absolute_population());
    let class_sample = fixture.sample("class-sample", &[class_d3()], class_population());
    let invalid_alpha = fixture.path("invalid-alpha");
    assert!(Pipeline::from_sample_dir(&sample)
        .filters()
        .run(&invalid_alpha, 0.0)
        .is_err());
    assert!(!invalid_alpha.exists());

    let mut cases = vec![
        Pipeline::from_sample_dir(&sample),
        Pipeline::sample(SampleSpec {
            population: Population::Absolute,
            discs: None,
            sample_size: 0,
            seed: 1,
        })
        .filters(),
        Pipeline::sample(SampleSpec {
            population: Population::Absolute,
            discs: Some(3),
            sample_size: 1,
            seed: 1,
        })
        .filters(),
    ];
    for stage in [
        SatStage {
            seeds: 0,
            ..sat_stage(SatEncoding::Ply)
        },
        SatStage {
            parallel_goals: 0,
            ..sat_stage(SatEncoding::Ply)
        },
        SatStage {
            timeout: Some(Duration::ZERO),
            ..sat_stage(SatEncoding::Ply)
        },
    ] {
        cases.push(Pipeline::from_sample_dir(&sample).filters().sat(stage));
    }
    cases.push(
        Pipeline::from_sample_dir(&sample)
            .filters()
            .reverse_search(Strategy::Dfs {
                discs: 3,
                max_nodes: 1,
            }),
    );
    cases
        .push(Pipeline::from_sample_dir(&class_sample).equivalence_ng(&fixture.path("unused-run")));
    for (index, case) in cases.into_iter().enumerate() {
        let output = fixture.path(&format!("invalid-{index}"));
        assert!(
            case.run(&output, ALPHA).is_err(),
            "invalid case {index} was accepted"
        );
        assert!(!output.exists(), "invalid case {index} created output");
    }
}

#[test]
fn invalid_sample_metadata_is_rejected_before_output() {
    for (field, value) in [
        ("uniform", json!(false)),
        ("schema_version", json!(2)),
        ("method", json!("random-play")),
        ("with_replacement", json!(false)),
        ("seed", json!(-1)),
        ("accepted_count", json!(2)),
    ] {
        let fixture = Fixture::new();
        let sample = fixture.sample("sample", &[initial()], absolute_population());
        let path = sample.join("sampling.json");
        let mut metadata = read_json(&path);
        metadata[field] = value;
        write_json(&path, &metadata);
        let output = fixture.path("run");
        assert!(Pipeline::from_sample_dir(&sample)
            .filters()
            .run(&output, ALPHA)
            .is_err());
        assert!(!output.exists());
    }

    let fixture = Fixture::new();
    let sample = fixture.sample("missing-input-count", &[initial()], class_population());
    let path = sample.join("sampling.json");
    let mut metadata = read_json(&path);
    metadata.as_object_mut().unwrap().remove("input_count");
    write_json(&path, &metadata);
    let output = fixture.path("missing-input-count-run");
    assert!(Pipeline::from_sample_dir(&sample)
        .filters()
        .run(&output, ALPHA)
        .is_err());
    assert!(!output.exists());

    let fixture = Fixture::new();
    let sample = fixture.sample(
        "small-input-count",
        &[initial(), initial()],
        class_population(),
    );
    let path = sample.join("sampling.json");
    let mut metadata = read_json(&path);
    metadata["input_count"] = json!(1);
    write_json(&path, &metadata);
    let output = fixture.path("small-input-count-run");
    assert!(Pipeline::from_sample_dir(&sample)
        .filters()
        .run(&output, ALPHA)
        .is_err());
    assert!(!output.exists());

    let fixture = Fixture::new();
    let sample = fixture.sample("empty-sample", &[], absolute_population());
    let output = fixture.path("empty-run");
    assert!(Pipeline::from_sample_dir(&sample)
        .filters()
        .run(&output, ALPHA)
        .is_err());
    assert!(!output.exists());
}

#[test]
fn generated_equivalence_sample_size_survives_resume_and_branch() {
    let fixture = Fixture::new();
    let run = fixture.path("run");
    let spec = SampleSpec {
        population: Population::Classes,
        discs: Some(4),
        sample_size: 64,
        seed: 20261107,
    };
    Pipeline::sample(spec.clone())
        .filters()
        .run(&run, ALPHA)
        .unwrap();
    let first = summary(&run);
    assert_eq!(first["sample"]["sample_size"], spec.sample_size);
    assert!(first["sample"]["rows"].as_u64().unwrap() < spec.sample_size as u64);
    assert_eq!(
        first["sample"]["source"]["generated"]["sample_size"],
        spec.sample_size
    );

    let completed = tree_files(&run.join("01-filters"));
    Pipeline::sample(spec.clone())
        .filters()
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(summary(&run)["sample"]["sample_size"], spec.sample_size);
    assert_eq!(tree_files(&run.join("01-filters")), completed);

    let branch = fixture.path("branch");
    Pipeline::from_run(&run, 1)
        .filters()
        .run(&branch, ALPHA)
        .unwrap();
    assert_eq!(summary(&branch)["sample"]["sample_size"], spec.sample_size);

    let metadata_path = run.join("sample/sampling.json");
    let mut metadata = read_json(&metadata_path);
    metadata["input_count"] = json!(spec.sample_size + 1);
    write_json(&metadata_path, &metadata);
    assert_error_without_changes(&branch, || {
        Pipeline::from_run(&run, 1).filters().run(&branch, ALPHA)
    });
    assert_error_without_changes(&run, || Pipeline::sample(spec).filters().run(&run, ALPHA));
}

#[test]
fn converted_sample_uses_its_rows_as_the_sample_size() {
    let fixture = Fixture::new();
    let equivalence_sample = fixture.path("equivalence-sample");
    generate(
        &equivalence_sample,
        Population::Classes,
        Some(4),
        512,
        20261110,
    )
    .unwrap();
    let source = read_sample_dir(&equivalence_sample).unwrap();
    let exact_sample = fixture.path("exact-sample");
    convert_classes(&equivalence_sample, &exact_sample, 20261111).unwrap();
    let converted = read_sample_dir(&exact_sample).unwrap();
    let input_count = converted.metadata["input_count"].as_u64().unwrap() as usize;
    assert_eq!(input_count, source.positions.len());
    assert_eq!(converted.sample_size, converted.positions.len());
    assert_ne!(converted.sample_size, input_count);

    let run = fixture.path("run");
    let report = Pipeline::from_sample_dir(&exact_sample)
        .filters()
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(
        report.interval.sample_size,
        converted.positions.len() as u64
    );
    let saved = summary(&run);
    assert_eq!(saved["sample"]["rows"], converted.positions.len());
    assert_eq!(saved["sample"]["sample_size"], converted.positions.len());
    assert_ne!(saved["sample"]["sample_size"], input_count);
}

#[test]
fn equivalence_ng_inherits_only_proven_unreachable_classes() {
    let fixture = Fixture::new();
    let source_one = fixture.path("source-one");
    let class_spec = SampleSpec {
        population: Population::Classes,
        discs: Some(5),
        sample_size: 4_096,
        seed: 20261107,
    };
    Pipeline::sample(class_spec.clone())
        .filters()
        .run(&source_one, ALPHA)
        .unwrap();
    let source_branch = fixture.path("source-branch");
    let source_branch_report = Pipeline::from_run(&source_one, 1)
        .reverse_search(complete_dfs())
        .run(&source_branch, ALPHA)
        .unwrap();
    let source_one_report = Pipeline::sample(class_spec)
        .filters()
        .reverse_search(complete_dfs())
        .run(&source_one, ALPHA)
        .unwrap();
    let class_sample = source_one.join("sample");
    assert_eq!(source_branch_report.unknown, source_one_report.unknown);
    assert_eq!(source_one_report.unknown, 0);
    assert!(!ng_outputs(&source_one.join("01-filters")).is_empty());
    assert!(
        !read_positions(&source_one.join("02-reverse-search/reverse_equivalence_NG.txt"))
            .is_empty()
    );
    assert!(
        !read_positions(&source_one.join("02-reverse-search/reverse_equivalence_OK.txt"))
            .is_empty()
    );

    let source_two = fixture.path("source-two");
    let source_two_report = Pipeline::from_sample_dir(&class_sample)
        .filters()
        .reverse_search(limited_dfs())
        .run(&source_two, ALPHA)
        .unwrap();
    assert!(source_two_report.unknown > 0);
    let source_two_unknown =
        read_positions(&source_two.join("02-reverse-search/reverse_equivalence_UNKNOWN.txt"))
            .into_iter()
            .collect::<HashSet<_>>();
    assert!(!source_two_unknown.is_empty());

    let exact_sample = fixture.path("exact-sample");
    convert_classes(&class_sample, &exact_sample, 20261108).unwrap();
    let converted = read_sample_dir(&exact_sample).unwrap();
    assert_eq!(converted.population, Population::Absolute);
    assert_eq!(converted.discs, Some(5));
    let mut seen = HashSet::new();
    let unique_exact = converted
        .positions
        .iter()
        .copied()
        .filter(|position| seen.insert(*position))
        .collect::<Vec<_>>();

    let source_one_ng = ng_outputs(&source_one);
    let source_two_ng = ng_outputs(&source_two);
    let expected_partition = |source_ng: &HashSet<Position>| {
        unique_exact
            .iter()
            .copied()
            .partition::<Vec<_>, _>(|position| source_ng.contains(&class_representative(*position)))
    };

    let carry_one = fixture.path("carry-one");
    let carry_one_report = Pipeline::from_sample_dir(&exact_sample)
        .equivalence_ng(&source_one)
        .run(&carry_one, ALPHA)
        .unwrap();
    let (expected_one_ng, expected_one_unknown) = expected_partition(&source_one_ng);
    assert_eq!(
        read_positions(&carry_one.join("01-equivalence-ng/equivalence_NG.txt")),
        expected_one_ng
    );
    assert_eq!(
        read_positions(&carry_one.join("01-equivalence-ng/equivalence_UNKNOWN.txt")),
        expected_one_unknown
    );
    let carry_one_summary = summary(&carry_one);
    assert_eq!(
        carry_one_summary["stages"][0]["stage"],
        json!({"kind": "equivalence-ng", "settings": {"run": source_one}})
    );
    assert_eq!(
        carry_one_summary["stages"][0]["directory"],
        "01-equivalence-ng"
    );

    let carry_branch = fixture.path("carry-branch");
    let carry_branch_report = Pipeline::from_sample_dir(&exact_sample)
        .equivalence_ng(&source_branch)
        .run(&carry_branch, ALPHA)
        .unwrap();
    assert_eq!(
        (
            carry_branch_report.ok,
            carry_branch_report.ng,
            carry_branch_report.unknown
        ),
        (
            carry_one_report.ok,
            carry_one_report.ng,
            carry_one_report.unknown
        )
    );
    assert_eq!(
        read_positions(&carry_branch.join("01-equivalence-ng/equivalence_NG.txt")),
        read_positions(&carry_one.join("01-equivalence-ng/equivalence_NG.txt"))
    );
    assert_eq!(
        read_positions(&carry_branch.join("01-equivalence-ng/equivalence_UNKNOWN.txt")),
        read_positions(&carry_one.join("01-equivalence-ng/equivalence_UNKNOWN.txt"))
    );

    let carry_two = fixture.path("carry-two");
    Pipeline::from_sample_dir(&exact_sample)
        .equivalence_ng(&source_two)
        .run(&carry_two, ALPHA)
        .unwrap();
    let (expected_two_ng, expected_two_unknown) = expected_partition(&source_two_ng);
    assert_eq!(
        read_positions(&carry_two.join("01-equivalence-ng/equivalence_NG.txt")),
        expected_two_ng
    );
    assert_eq!(
        read_positions(&carry_two.join("01-equivalence-ng/equivalence_UNKNOWN.txt")),
        expected_two_unknown
    );
    let inherited_source_unknown = unique_exact
        .iter()
        .copied()
        .filter(|position| source_two_unknown.contains(&class_representative(*position)))
        .collect::<Vec<_>>();
    assert!(!inherited_source_unknown.is_empty());
    let carried_two_unknown =
        read_positions(&carry_two.join("01-equivalence-ng/equivalence_UNKNOWN.txt"))
            .into_iter()
            .collect::<HashSet<_>>();
    assert!(inherited_source_unknown
        .iter()
        .all(|position| carried_two_unknown.contains(position)));

    let carry_complete = fixture.path("carry-complete");
    let carry_complete_report = Pipeline::from_sample_dir(&exact_sample)
        .equivalence_ng(&source_one)
        .reverse_search(complete_dfs())
        .run(&carry_complete, ALPHA)
        .unwrap();
    let baseline = fixture.path("baseline");
    let baseline_report = Pipeline::from_sample_dir(&exact_sample)
        .reverse_search(complete_dfs())
        .run(&baseline, ALPHA)
        .unwrap();
    assert_eq!(
        (
            carry_complete_report.ok,
            carry_complete_report.ng,
            carry_complete_report.unknown
        ),
        (
            baseline_report.ok,
            baseline_report.ng,
            baseline_report.unknown
        )
    );
    let position_set = |path: &Path| read_positions(path).into_iter().collect::<HashSet<_>>();
    assert_eq!(
        position_set(&carry_complete.join("02-reverse-search/reverse_exact_OK.txt")),
        position_set(&baseline.join("01-reverse-search/reverse_exact_OK.txt"))
    );
    let mut carried_ng = position_set(&carry_complete.join("01-equivalence-ng/equivalence_NG.txt"));
    carried_ng.extend(position_set(
        &carry_complete.join("02-reverse-search/reverse_exact_NG.txt"),
    ));
    assert_eq!(
        carried_ng,
        position_set(&baseline.join("01-reverse-search/reverse_exact_NG.txt"))
    );
    assert_eq!(
        position_set(&carry_complete.join("02-reverse-search/reverse_exact_UNKNOWN.txt")),
        position_set(&baseline.join("01-reverse-search/reverse_exact_UNKNOWN.txt"))
    );

    let incomplete = fixture.path("incomplete-source");
    copy_tree(&source_one, &incomplete);
    let mut incomplete_summary = summary(&incomplete);
    incomplete_summary["stages"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap()["result"] = Value::Null;
    write_json(&incomplete.join("summary.json"), &incomplete_summary);
    assert_equivalence_ng_runtime_error(
        &exact_sample,
        &incomplete,
        &fixture.path("incomplete-error"),
    );
    assert_equivalence_ng_runtime_error(
        &exact_sample,
        &baseline,
        &fixture.path("exact-source-error"),
    );

    let six_disc_sample = fixture.path("six-disc-sample");
    generate(
        &six_disc_sample,
        Population::Classes,
        Some(6),
        128,
        20261109,
    )
    .unwrap();
    let six_disc_source = fixture.path("six-disc-source");
    Pipeline::from_sample_dir(&six_disc_sample)
        .filters()
        .run(&six_disc_source, ALPHA)
        .unwrap();
    assert_equivalence_ng_runtime_error(
        &exact_sample,
        &six_disc_source,
        &fixture.path("six-disc-error"),
    );

    let unrestricted_sample = fixture.sample(
        "unrestricted-class-sample",
        &[class_d3()],
        class_population(),
    );
    let unrestricted_source = fixture.path("unrestricted-source");
    Pipeline::from_sample_dir(&unrestricted_sample)
        .filters()
        .run(&unrestricted_source, ALPHA)
        .unwrap();
    assert_equivalence_ng_runtime_error(
        &exact_sample,
        &unrestricted_source,
        &fixture.path("unrestricted-error"),
    );

    let missing_sample = fixture.sample(
        "missing-class-sample",
        &[class_d3()],
        fixed_classes(5, "1920"),
    );
    let missing_source = fixture.path("missing-source");
    Pipeline::from_sample_dir(&missing_sample)
        .filters()
        .run(&missing_source, ALPHA)
        .unwrap();
    assert!(unique_exact
        .iter()
        .any(|position| class_representative(*position) != class_d3()));
    assert_equivalence_ng_runtime_error(
        &exact_sample,
        &missing_source,
        &fixture.path("missing-error"),
    );

    let completed_files = tree_files(&carry_one.join("01-equivalence-ng"));
    fs::remove_dir_all(&source_one).unwrap();
    let resumed = Pipeline::from_sample_dir(&exact_sample)
        .equivalence_ng(&source_one)
        .run(&carry_one, ALPHA)
        .unwrap();
    assert_eq!(
        (resumed.ok, resumed.ng, resumed.unknown),
        (
            carry_one_report.ok,
            carry_one_report.ng,
            carry_one_report.unknown
        )
    );
    assert_eq!(
        tree_files(&carry_one.join("01-equivalence-ng")),
        completed_files
    );
}

#[test]
fn summary_is_single_atomic_run_record_with_recomputed_counts_and_intervals() {
    let fixture = Fixture::new();
    let sample = fixture.sample(
        "sample",
        &[disconnected(), disconnected(), initial(), after_d3()],
        absolute_population(),
    );
    let run = fixture.path("run");
    Pipeline::from_sample_dir(&sample)
        .filters()
        .reverse_search(limited_dfs())
        .run(&run, ALPHA)
        .unwrap();
    let summaries = tree_files(&run)
        .into_iter()
        .filter(|(path, _)| path.file_name().unwrap() == "summary.json")
        .collect::<Vec<_>>();
    assert_eq!(summaries.len(), 1);
    assert!(!run.join("summary.json.tmp").exists());

    let saved = summary(&run);
    assert_eq!(saved["sample"]["rows"], 4);
    assert_eq!(saved["sample"]["sample_size"], 4);
    assert_eq!(saved["stages"].as_array().unwrap().len(), 2);
    assert_eq!(saved["stages"][0]["stage"]["kind"], "filters");
    assert_eq!(saved["stages"][1]["stage"]["kind"], "reverse-search");
    assert!(saved["stages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|stage| stage["result"].is_object()));
    for stage in saved["stages"].as_array().unwrap() {
        let result = &stage["result"];
        let counts = &result["counts"];
        let expected = interval::compute(
            counts["OK"].as_u64().unwrap(),
            counts["NG"].as_u64().unwrap(),
            counts["UNKNOWN"].as_u64().unwrap(),
            NonZeroU128::new(ABSOLUTE_SIZE).unwrap(),
            ALPHA,
        )
        .unwrap();
        close(
            result["wilson_interval"][0].as_f64().unwrap(),
            expected.lower,
        );
        close(
            result["wilson_interval"][1].as_f64().unwrap(),
            expected.upper,
        );
        assert_eq!(
            result["reachable_interval"],
            json!([
                expected.expected_lower.to_string(),
                expected.expected_upper.to_string()
            ])
        );
    }
}

#[test]
fn runtime_failure_leaves_declared_stages_incomplete_and_resume_runs_only_tail() {
    let fixture = Fixture::new();
    let sample = fixture.sample("sample", &[initial(), after_d3()], absolute_population());

    let first_tmp = fixture.path("first-tmp");
    fs::create_dir(&first_tmp).unwrap();
    fs::write(first_tmp.join("junk"), b"x").unwrap();
    let first_run = fixture.path("first-run");
    assert!(Pipeline::from_sample_dir(&sample)
        .reverse_search(external(first_tmp, false))
        .filters()
        .run(&first_run, ALPHA)
        .is_err());
    let first_summary = summary(&first_run);
    assert!(first_summary["stages"][0]["result"].is_null());
    assert!(first_summary["stages"][1]["result"].is_null());

    let later_tmp = fixture.path("later-tmp");
    fs::create_dir(&later_tmp).unwrap();
    fs::write(later_tmp.join("junk"), b"x").unwrap();
    let later_run = fixture.path("later-run");
    assert!(Pipeline::from_sample_dir(&sample)
        .filters()
        .reverse_search(external(later_tmp.clone(), false))
        .run(&later_run, ALPHA)
        .is_err());
    let failed = summary(&later_run);
    assert!(failed["stages"][0]["result"].is_object());
    assert!(failed["stages"][1]["result"].is_null());
    let first_elapsed = failed["stages"][0]["result"]["elapsed_secs"].clone();
    let first_files = tree_files(&later_run.join("01-filters"));

    fs::remove_file(later_tmp.join("junk")).unwrap();
    Pipeline::from_sample_dir(&sample)
        .filters()
        .reverse_search(external(later_tmp, false))
        .run(&later_run, ALPHA)
        .unwrap();
    let resumed = summary(&later_run);
    assert_eq!(
        resumed["stages"][0]["result"]["elapsed_secs"],
        first_elapsed
    );
    assert_eq!(tree_files(&later_run.join("01-filters")), first_files);
    assert!(resumed["stages"][1]["result"].is_object());
}

#[test]
fn resume_accepts_a_matching_prefix_and_alpha_only_changes_intervals() {
    let fixture = Fixture::new();
    let sample = fixture.sample("sample", &[initial(), after_d3()], absolute_population());
    let run = fixture.path("run");
    Pipeline::from_sample_dir(&sample)
        .filters()
        .run(&run, ALPHA)
        .unwrap();
    let first_elapsed = summary(&run)["stages"][0]["result"]["elapsed_secs"].clone();
    let filter_files = tree_files(&run.join("01-filters"));

    Pipeline::from_sample_dir(&sample)
        .filters()
        .sat(sat_stage(SatEncoding::Ply))
        .run(&run, ALPHA)
        .unwrap();
    let extended = summary(&run);
    assert_eq!(
        extended["stages"][0]["result"]["elapsed_secs"],
        first_elapsed
    );
    assert!(extended["stages"][1]["result"].is_object());
    assert_eq!(tree_files(&run.join("01-filters")), filter_files);
    let old_bound = extended["stages"][1]["result"]["wilson_interval"][0]
        .as_f64()
        .unwrap();
    let all_stage_files = [
        tree_files(&run.join("01-filters")),
        tree_files(&run.join("02-sat")),
    ];

    Pipeline::from_sample_dir(&sample)
        .filters()
        .sat(sat_stage(SatEncoding::Ply))
        .run(&run, 0.05)
        .unwrap();
    let changed = summary(&run);
    assert_eq!(changed["alpha"], 0.05);
    assert_ne!(
        changed["stages"][1]["result"]["wilson_interval"][0]
            .as_f64()
            .unwrap(),
        old_bound
    );
    assert_eq!(tree_files(&run.join("01-filters")), all_stage_files[0]);
    assert_eq!(tree_files(&run.join("02-sat")), all_stage_files[1]);
}

#[test]
fn changed_generated_spec_or_completed_settings_fails_without_modification() {
    let fixture = Fixture::new();
    let run = fixture.path("generated");
    let spec = SampleSpec {
        population: Population::Absolute,
        discs: Some(4),
        sample_size: 2,
        seed: 7,
    };
    Pipeline::sample(spec.clone())
        .filters()
        .run(&run, ALPHA)
        .unwrap();
    assert_error_without_changes(&run, || {
        Pipeline::sample(SampleSpec {
            seed: 8,
            ..spec.clone()
        })
        .filters()
        .run(&run, ALPHA)
    });

    let sample = fixture.sample("sample", &[after_d3()], absolute_population());
    let reverse_run = fixture.path("reverse");
    Pipeline::from_sample_dir(&sample)
        .reverse_search(limited_dfs())
        .run(&reverse_run, ALPHA)
        .unwrap();
    assert_error_without_changes(&reverse_run, || {
        Pipeline::from_sample_dir(&sample)
            .reverse_search(complete_dfs())
            .run(&reverse_run, ALPHA)
    });
}

#[test]
fn resume_rejects_partition_record_and_nopass_corruption_without_changes() {
    let fixture = Fixture::new();

    let filter_sample = fixture.sample(
        "filter-sample",
        &[initial(), after_d3()],
        absolute_population(),
    );
    let filter_run = fixture.path("filter-run");
    Pipeline::from_sample_dir(&filter_sample)
        .filters()
        .run(&filter_run, ALPHA)
        .unwrap();
    write_positions(&filter_run.join("01-filters/con_OK.txt"), &[initial()]);
    assert_error_without_changes(&filter_run, || {
        Pipeline::from_sample_dir(&filter_sample)
            .filters()
            .run(&filter_run, ALPHA)
    });

    let sat_sample = fixture.sample(
        "sat-sample",
        &[initial(), after_d3()],
        absolute_population(),
    );
    let sat_run = fixture.path("sat-run");
    Pipeline::from_sample_dir(&sat_sample)
        .sat(sat_stage(SatEncoding::Ply))
        .run(&sat_run, ALPHA)
        .unwrap();
    let path = sat_run.join("01-sat/sat_exact_records.csv");
    let mut records = read_records(&path);
    records[1].input_index = 1;
    write_records(&path, &records);
    assert_error_without_changes(&sat_run, || {
        Pipeline::from_sample_dir(&sat_sample)
            .sat(sat_stage(SatEncoding::Ply))
            .run(&sat_run, ALPHA)
    });

    let witness_sample = fixture.sample("witness-sample", &[class_d3()], absolute_population());
    let witness_run = fixture.path("witness-run");
    Pipeline::from_sample_dir(&witness_sample)
        .sat(sat_stage(SatEncoding::NoPassWitness))
        .run(&witness_run, ALPHA)
        .unwrap();
    write_positions(&witness_run.join("01-sat/sat_exact_UNKNOWN.txt"), &[]);
    write_positions(&witness_run.join("01-sat/sat_exact_NG.txt"), &[class_d3()]);
    assert_error_without_changes(&witness_run, || {
        Pipeline::from_sample_dir(&witness_sample)
            .sat(sat_stage(SatEncoding::NoPassWitness))
            .run(&witness_run, ALPHA)
    });
}

#[test]
fn every_reverse_strategy_proves_known_exact_and_equivalence_positions() {
    for (stem, positions, population, mode) in [
        (
            "exact",
            [initial(), after_d3()],
            absolute_population(),
            GoalMatch::Exact,
        ),
        (
            "equivalence",
            [initial(), class_d3()],
            class_population(),
            GoalMatch::EquivalenceClass,
        ),
    ] {
        let fixture = Fixture::new();
        let sample = fixture.sample("sample", &positions, population);
        for strategy in small_strategies(&fixture, stem) {
            let run = fixture.path(&format!("run-{}", strategy.name()));
            Pipeline::from_sample_dir(&sample)
                .reverse_search(strategy.clone())
                .run(&run, ALPHA)
                .unwrap();
            let prefix = format!("reverse_{}", mode.as_str());
            assert_eq!(
                read_positions(&run.join(format!("01-reverse-search/{prefix}_OK.txt"))),
                positions,
                "{} failed in {stem} mode",
                strategy.name()
            );
            assert!(
                read_positions(&run.join(format!("01-reverse-search/{prefix}_NG.txt"))).is_empty()
            );
            let records =
                read_records(&run.join(format!("01-reverse-search/{prefix}_records.csv")));
            assert_eq!(records.len(), 2);
            assert_eq!(
                records
                    .iter()
                    .map(|record| record.goal)
                    .collect::<HashSet<_>>(),
                positions.into_iter().collect()
            );
            assert!(records.iter().all(|record| record.goal_match == mode));
            for record in records {
                record.verify().unwrap();
            }
        }
    }
}

#[test]
fn external_bfs_resumes_only_the_same_input_and_settings() {
    let fixture = Fixture::new();
    let sample = fixture.sample("sample", &[initial(), after_d3()], absolute_population());
    let tmp = fixture.path("tmp");
    fs::create_dir(&tmp).unwrap();
    let first = fixture.path("first");
    Pipeline::from_sample_dir(&sample)
        .reverse_search(external(tmp.clone(), false))
        .run(&first, ALPHA)
        .unwrap();
    let resumed = fixture.path("resumed");
    Pipeline::from_sample_dir(&sample)
        .reverse_search(external(tmp.clone(), true))
        .run(&resumed, ALPHA)
        .unwrap();
    assert_eq!(
        tree_files(&first.join("01-reverse-search")),
        tree_files(&resumed.join("01-reverse-search"))
    );

    let changed = fixture.path("changed");
    let mut config = match external(tmp.clone(), true) {
        Strategy::BfsExternal(config) => config,
        _ => unreachable!(),
    };
    config.block_size = 5;
    assert!(Pipeline::from_sample_dir(&sample)
        .reverse_search(Strategy::BfsExternal(config))
        .run(&changed, ALPHA)
        .is_err());

    let other_sample = fixture.sample("other-sample", &[class_d3()], absolute_population());
    assert!(Pipeline::from_sample_dir(&other_sample)
        .reverse_search(external(tmp, true))
        .run(&fixture.path("other"), ALPHA)
        .is_err());
}

#[test]
fn reverse_ng_updates_duplicate_rows_and_nodes_cover_each_stage_input() {
    let fixture = Fixture::new();
    let sample = fixture.sample(
        "sample",
        &[after_d3(), class_d3(), class_d3()],
        fixed_absolute(5, "3840"),
    );
    let run = fixture.path("run");
    let report = Pipeline::from_sample_dir(&sample)
        .reverse_search(complete_dfs())
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(
        read_positions(&run.join("01-reverse-search/reverse_exact_OK.txt")),
        [after_d3()]
    );
    assert_eq!(
        read_positions(&run.join("01-reverse-search/reverse_exact_NG.txt")),
        [class_d3()]
    );
    assert_eq!((report.ok, report.ng, report.unknown), (1, 2, 0));
    let nodes = summary(&run)["stages"][0]["result"]["nodes"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(nodes.len(), 2);
    assert!(nodes.iter().all(|count| count.as_u64().unwrap() > 0));
}

#[test]
fn existing_directories_keep_unrelated_files_and_inputs_cannot_be_outputs() {
    let fixture = Fixture::new();
    let sample = fixture.sample("sample", &[initial()], absolute_population());
    let before_sample = tree_files(&sample);
    assert!(Pipeline::from_sample_dir(&sample)
        .filters()
        .run(&sample, ALPHA)
        .is_err());
    assert_eq!(tree_files(&sample), before_sample);

    let run = fixture.path("run");
    fs::create_dir_all(run.join("01-filters")).unwrap();
    fs::write(run.join("keep.txt"), b"root").unwrap();
    fs::write(run.join("01-filters/keep.txt"), b"stage").unwrap();
    Pipeline::from_sample_dir(&sample)
        .filters()
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(fs::read(run.join("keep.txt")).unwrap(), b"root");
    assert_eq!(fs::read(run.join("01-filters/keep.txt")).unwrap(), b"stage");
}

#[test]
fn from_run_references_its_source_without_copying_inherited_files() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    let spec = SampleSpec {
        population: Population::Absolute,
        discs: Some(5),
        sample_size: 64,
        seed: 20261022,
    };
    Pipeline::sample(spec.clone())
        .filters()
        .reverse_search(limited_dfs())
        .run(&source, ALPHA)
        .unwrap();
    let source_before = tree_files(&source);
    let source_argument = source.join("..").join("source");

    let branch = fixture.path("branch");
    Pipeline::from_run(&source_argument, 2)
        .filters()
        .run(&branch, ALPHA)
        .unwrap();
    assert_eq!(tree_files(&source), source_before);
    assert_eq!(
        directory_entries(&branch),
        HashSet::from(["03-filters".to_owned(), "summary.json".to_owned()])
    );
    assert!(!branch.join("sample").exists());
    assert!(!branch.join("01-filters").exists());
    assert!(!branch.join("02-reverse-search").exists());
    assert!(summary(&branch)["stages"][2]["result"].is_object());
    assert_eq!(
        summary(&branch)["branched_from"],
        json!({"run": source_argument, "stages": 2})
    );

    let inherited_interval = summary(&branch)["stages"][1]["result"]["wilson_interval"].clone();
    Pipeline::from_run(&source_argument, 2)
        .filters()
        .run(&branch, 0.05)
        .unwrap();
    let changed = summary(&branch);
    assert_eq!(changed["alpha"], 0.05);
    assert_ne!(
        changed["stages"][1]["result"]["wilson_interval"],
        inherited_interval
    );
    Pipeline::from_run(&source_argument, 2)
        .filters()
        .run(&branch, 0.05)
        .unwrap();

    Pipeline::sample(spec)
        .filters()
        .reverse_search(limited_dfs())
        .filters()
        .run(&source, ALPHA)
        .unwrap();
    Pipeline::from_run(&source_argument, 2)
        .filters()
        .run(&branch, 0.05)
        .unwrap();

    assert_error_without_changes(&source, || {
        Pipeline::from_run(&source_argument, 2)
            .filters()
            .run(&source, 0.05)
    });
    assert_error_without_changes(&source, || {
        Pipeline::from_run(&source_argument, 2)
            .filters()
            .run(&source.join("sample"), 0.05)
    });

    let source_summary = summary(&source);
    let mut changed_source = source_summary.clone();
    changed_source["stages"][1]["stage"]["settings"]["Dfs"]["max_nodes"] = json!(1);
    write_json(&source.join("summary.json"), &changed_source);
    assert_error_without_changes(&branch, || {
        Pipeline::from_run(&source_argument, 2)
            .filters()
            .run(&branch, 0.05)
    });
    write_json(&source.join("summary.json"), &source_summary);

    fs::remove_dir_all(&source).unwrap();
    assert_error_without_changes(&branch, || {
        Pipeline::from_run(&source_argument, 2)
            .filters()
            .run(&branch, 0.05)
    });
}

#[test]
fn nested_branches_follow_all_ancestors_and_reject_cycles() {
    let fixture = Fixture::new();
    let spec = SampleSpec {
        population: Population::Absolute,
        discs: Some(4),
        sample_size: 256,
        seed: 20261111,
    };
    let run_a = fixture.path("run-a");
    let report_a = Pipeline::sample(spec.clone())
        .filters()
        .run(&run_a, ALPHA)
        .unwrap();
    assert!(report_a.unknown > 0);

    let run_b = fixture.path("run-b");
    let report_b = Pipeline::from_run(&run_a, 1)
        .reverse_search(complete_dfs())
        .run(&run_b, ALPHA)
        .unwrap();
    assert!(report_b.unknown < report_a.unknown);

    let run_c = fixture.path("run-c");
    let report_c = Pipeline::from_run(&run_b, 2)
        .filters()
        .run(&run_c, ALPHA)
        .unwrap();
    assert_eq!(
        directory_entries(&run_c),
        HashSet::from(["03-filters".to_owned(), "summary.json".to_owned()])
    );

    let baseline = fixture.path("baseline");
    let baseline_report = Pipeline::sample(spec)
        .filters()
        .reverse_search(complete_dfs())
        .filters()
        .run(&baseline, ALPHA)
        .unwrap();
    assert_eq!(
        (report_c.ok, report_c.ng, report_c.unknown),
        (
            baseline_report.ok,
            baseline_report.ng,
            baseline_report.unknown
        )
    );

    let mut summary_a = summary(&run_a);
    summary_a["branched_from"] = json!({"run": run_c, "stages": 1});
    write_json(&run_a.join("summary.json"), &summary_a);
    assert_error_without_changes(&run_c, || {
        Pipeline::from_run(&run_b, 2).filters().run(&run_c, ALPHA)
    });
}
