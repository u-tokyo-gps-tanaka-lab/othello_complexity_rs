//! Integration tests for the in-process estimation pipeline.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::num::NonZeroU128;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use othello_complexity_rs::estimate::{Pipeline, SampleSpec, SatStage};
use othello_complexity_rs::filters::sat::{ReverseBfs, SatEncoding};
use othello_complexity_rs::interval;
use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{Board, Color, Position};
use othello_complexity_rs::record::{GoalMatch, PlyAction, PositionTransform, ReachabilityRecord};
use othello_complexity_rs::sampling::Population;
use othello_complexity_rs::search::run::{ExternalBfs, Strategy};
use serde_json::{json, Value};

// P(|Z| > 1). With z² = 1, an all-OK sample of size n has bounds
// [n / (n + 1), 1].
const ALPHA: f64 = 0.31731050786291415;
const ABSOLUTE_SIZE: u128 = 32 * 3_u128.pow(60);
const CLASS_SIZE: &str = "84782316550443833986427293875";
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
                "input_count": positions.len(),
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
        "definition": "equivalence_classes_center_occupied",
        "size": CLASS_SIZE
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
        "definition": "equivalence_classes_fixed_discs",
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
        reverse_bfs: None,
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

fn assert_error_without_changes<T>(directory: &Path, operation: impl FnOnce() -> io::Result<T>) {
    let before = tree_files(directory);
    assert!(operation().is_err());
    assert_eq!(tree_files(directory), before);
}

fn summary(run: &Path) -> Value {
    read_json(&run.join("summary.json"))
}

fn small_strategies(fixture: &Fixture, stem: &str) -> Vec<Strategy> {
    let sequential_tmp = fixture.path(&format!("{stem}-bfs-tmp"));
    let parallel_tmp = fixture.path(&format!("{stem}-bfs-parallel-tmp"));
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
        Strategy::BfsInmemoryParallel {
            discs: 4,
            max_nodes: 128,
            use_lp: false,
            use_occupancy_cache: false,
            threads: 2,
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
    for filter in ["edge", "occupancy", "seg3more", "lp"] {
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
fn duplicate_rows_keep_weight_for_three_populations() {
    for (position, population, mode, size) in [
        (initial(), fixed_absolute(4, "32"), "exact", 32.0),
        (initial(), fixed_classes(4, "6"), "equivalence", 6.0),
        (after_d3(), fixed_absolute(5, "3840"), "exact", 3840.0),
    ] {
        let fixture = Fixture::new();
        let sample = fixture.sample("sample", &[position; 3], population);
        let run = fixture.path("run");
        let report = Pipeline::from_sample_dir(&sample)
            .sat(sat_stage(SatEncoding::Ply))
            .run(&run, ALPHA)
            .unwrap();
        assert_eq!((report.ok, report.ng, report.unknown), (3, 0, 0));
        close(report.interval.lower, 0.75);
        close(report.interval.upper, 1.0);
        close(
            report.interval.expected_lower.to_string().parse().unwrap(),
            0.75 * size,
        );
        let records = read_records(&run.join(format!("01-sat/sat_{mode}_records.csv")));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_index, 1);
        assert_eq!(records[0].goal, position);
        records[0].verify().unwrap();
    }
}

#[test]
fn black_turn_d3_is_exact_ng_but_equivalence_ok() {
    for (population, mode, expected) in [
        (fixed_absolute(5, "3840"), "exact", "NG"),
        (fixed_classes(5, "264"), "equivalence", "OK"),
    ] {
        let fixture = Fixture::new();
        let sample = fixture.sample("sample", &[class_d3()], population);
        let run = fixture.path("run");
        Pipeline::from_sample_dir(&sample)
            .sat(sat_stage(SatEncoding::Ply))
            .run(&run, ALPHA)
            .unwrap();
        assert_eq!(
            fs::read_to_string(run.join(format!("01-sat/sat_{mode}_{expected}.txt"))).unwrap(),
            position_text(&[class_d3()])
        );
        let records = read_records(&run.join(format!("01-sat/sat_{mode}_records.csv")));
        if expected == "OK" {
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].goal_match, GoalMatch::EquivalenceClass);
            records[0].verify().unwrap();
        } else {
            assert!(records.is_empty());
        }
    }
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
fn stage_kinds_can_repeat_and_run_in_every_required_order() {
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

    let run = fixture.path("reverse-reverse");
    Pipeline::from_sample_dir(&sample)
        .reverse_search(limited_dfs())
        .reverse_search(complete_dfs())
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(summary(&run)["stages"][1]["result"]["input"], 1);
    assert_eq!(
        read_positions(&run.join("02-reverse-search/reverse_exact_OK.txt")),
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

    let sample = fixture.sample(
        "filters-filters-sample",
        &[disconnected(), after_d3()],
        absolute_population(),
    );
    let run = fixture.path("filters-filters");
    Pipeline::from_sample_dir(&sample)
        .filters()
        .filters()
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(summary(&run)["stages"][1]["result"]["input"], 1);
    assert_eq!(
        read_positions(&run.join("02-filters/con_OK.txt")),
        [after_d3()]
    );

    let run = fixture.path("no-filters");
    Pipeline::from_sample_dir(&sample)
        .sat(sat_stage(SatEncoding::Ply))
        .run(&run, ALPHA)
        .unwrap();
    assert_eq!(summary(&run)["stages"][0]["stage"]["kind"], "sat");
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
            count: 0,
            seed: 1,
        })
        .filters(),
        Pipeline::sample(SampleSpec {
            population: Population::Absolute,
            discs: Some(3),
            count: 1,
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
        SatStage {
            reverse_bfs: Some(ReverseBfs {
                discs: 4,
                state_budget: 0,
            }),
            ..sat_stage(SatEncoding::Ply)
        },
        SatStage {
            reverse_bfs: Some(ReverseBfs {
                discs: 3,
                state_budget: 1,
            }),
            ..sat_stage(SatEncoding::Ply)
        },
        SatStage {
            reverse_bfs: Some(ReverseBfs {
                discs: 4,
                state_budget: 1,
            }),
            ..sat_stage(SatEncoding::NoPassWitness)
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
    let tmp = fixture.path("unused-tmp");
    cases.push(
        Pipeline::from_sample_dir(&sample)
            .filters()
            .reverse_search(Strategy::BfsExternal(ExternalBfs {
                discs: 4,
                jobs: 1,
                verbose: 0,
                block_size: 0,
                tmp_dir: tmp,
                resume: false,
            })),
    );

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
    let sample = fixture.sample("empty-sample", &[], absolute_population());
    let output = fixture.path("empty-run");
    assert!(Pipeline::from_sample_dir(&sample)
        .filters()
        .run(&output, ALPHA)
        .is_err());
    assert!(!output.exists());
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
    assert_eq!(saved["stages"].as_array().unwrap().len(), 2);
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
        count: 2,
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
fn from_run_copies_prefix_without_touching_source_and_survives_source_deletion() {
    let fixture = Fixture::new();
    let source = fixture.path("source");
    let spec = SampleSpec {
        population: Population::Absolute,
        discs: Some(4),
        count: 4,
        seed: 20261022,
    };
    Pipeline::sample(spec)
        .filters()
        .run(&source, ALPHA)
        .unwrap();
    let source_before = tree_files(&source);
    let source_sample = tree_files(&source.join("sample"));
    let source_stage = tree_files(&source.join("01-filters"));
    let source_argument = source.join("..").join("source");

    let branch = fixture.path("branch");
    Pipeline::from_run(&source_argument, 1)
        .filters()
        .run(&branch, ALPHA)
        .unwrap();
    assert_eq!(tree_files(&source), source_before);
    assert_eq!(tree_files(&branch.join("sample")), source_sample);
    assert_eq!(tree_files(&branch.join("01-filters")), source_stage);
    assert!(summary(&branch)["stages"][1]["result"].is_object());
    assert_eq!(
        summary(&branch)["branched_from"],
        json!({"run": source_argument, "stages": 1})
    );

    fs::remove_dir_all(&source).unwrap();
    Pipeline::from_run(&source_argument, 1)
        .filters()
        .run(&branch, 0.05)
        .unwrap();
    assert_eq!(summary(&branch)["alpha"], 0.05);
}
