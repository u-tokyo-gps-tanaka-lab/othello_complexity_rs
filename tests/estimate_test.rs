//! Integration tests for the staged estimation contract.
//!
//! Expected classifications come from sample multiplicity, the initial
//! position, Black's legal opening D3, and the connectivity filter. Corruption
//! tests start from directories written by the public stage functions.

use othello_complexity_rs::estimate;
use othello_complexity_rs::filters::sat::SatEncoding;
use othello_complexity_rs::interval;
use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{Board, Color, Position};
use othello_complexity_rs::record::{GoalMatch, PlyAction, PositionTransform, ReachabilityRecord};
use othello_complexity_rs::search::run::{ExternalBfs, Strategy};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::num::NonZeroU128;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

// P(|Z| > 1) from the standard normal integral. With z^2 = 1, an all-OK
// sample of size n has Wilson bounds [n / (n + 1), 1].
const ALPHA: f64 = 0.31731050786291415;
// Four occupied central cells, 60 ternary cells, and two absolute turns.
const ABSOLUTE_SIZE: u128 = 32 * 3_u128.pow(60);
const CLASS_SIZE: &str = "84782316550443833986427293875";
static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "othello-estimate-stages-test-{}-{}",
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
        let dir = self.path(name);
        fs::create_dir(&dir).unwrap();
        write_positions(&dir.join("positions.txt"), positions);
        write_json(
            &dir.join("sampling.json"),
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
        dir
    }

    fn filters(
        &self,
        sample_name: &str,
        output_name: &str,
        positions: &[Position],
        population: Value,
    ) -> PathBuf {
        let sample = self.sample(sample_name, positions, population);
        let output = self.path(output_name);
        estimate::filters(&sample, &output, ALPHA).unwrap();
        output
    }

    fn unknown_reverse(&self) -> (PathBuf, PathBuf) {
        let filters = self.filters(
            "sample",
            "filters",
            &[initial(), after_d3()],
            absolute_population(),
        );
        let output = self.path("reverse-limited");
        estimate::reverse_search(&filters, &output, ALPHA, &limited_dfs()).unwrap();
        (filters, output)
    }

    fn sat_ok(&self) -> (PathBuf, PathBuf) {
        let filters = self.filters("sample", "filters", &[after_d3()], absolute_population());
        let output = self.path("sat");
        estimate::sat(&filters, &output, ALPHA, SatEncoding::Ply, 1, None).unwrap();
        (filters, output)
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
    // White owns E5; Black owns D3, D4, E4 and D5. White moves next.
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
    // An isolated A1 disc cannot have been placed by a legal move.
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

fn write_positions(path: &Path, positions: &[Position]) {
    fs::write(path, position_text(positions)).unwrap();
}

fn position_text(positions: &[Position]) -> String {
    positions
        .iter()
        .map(|&position| format!("{}\n", format_position(position)))
        .collect()
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

fn saved_sat_with_unknown(fixture: &Fixture, filters: &Path, encoding: SatEncoding) -> PathBuf {
    let directory = fixture.path("saved-sat");
    fs::create_dir(&directory).unwrap();
    write_positions(&directory.join("sat_exact_OK.txt"), &[initial()]);
    write_positions(&directory.join("sat_exact_NG.txt"), &[]);
    write_positions(&directory.join("sat_exact_UNKNOWN.txt"), &[after_d3()]);
    write_records(
        &directory.join("sat_exact_records.csv"),
        &[initial_record(1)],
    );

    let confidence =
        interval::compute(1, 1, 2, NonZeroU128::new(ABSOLUTE_SIZE).unwrap(), ALPHA).unwrap();
    write_json(
        &directory.join("summary.json"),
        &json!({
            "stage": format!("sat:{encoding}"),
            "from": filters.canonicalize().unwrap(),
            "population": absolute_population(),
            "mode": "exact",
            "with_replacement": true,
            "accepted_samples": 4,
            "counts": {"OK": 1, "NG": 1, "UNKNOWN": 2},
            "alpha": ALPHA,
            "wilson_interval": [confidence.lower, confidence.upper],
            "reachable_interval": [
                confidence.expected_lower.to_string(),
                confidence.expected_upper.to_string(),
            ],
        }),
    );
    directory
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}

fn assert_summary_keys(summary: &Value, expected: &[&str]) {
    let actual: BTreeSet<_> = summary
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let expected: BTreeSet<_> = expected.iter().copied().collect();
    assert_eq!(actual, expected);
}

fn base_summary_keys() -> Vec<&'static str> {
    vec![
        "stage",
        "from",
        "population",
        "mode",
        "with_replacement",
        "accepted_samples",
        "counts",
        "alpha",
        "wilson_interval",
        "reachable_interval",
    ]
}

fn filter_summary_keys() -> Vec<&'static str> {
    let mut keys = base_summary_keys();
    keys.push("sample_dir");
    keys
}

fn reverse_summary_keys() -> Vec<&'static str> {
    let mut keys = base_summary_keys();
    keys.push("nodes");
    keys
}

fn file_names(directory: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn assert_stage_rejected(fixture: &Fixture, from: &Path, output_name: &str) {
    let output = fixture.path(output_name);
    assert!(estimate::reverse_search(from, &output, ALPHA, &limited_dfs()).is_err());
    assert!(!output.exists());
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
        Strategy::BfsExternal(ExternalBfs {
            discs: 4,
            jobs: 2,
            verbose: 0,
            block_size: 4,
            tmp_dir: sequential_tmp,
            resume: false,
        }),
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
fn filters_match_the_deduplicated_reference_pipeline_and_write_only_stage_files() {
    let fixture = Fixture::new();
    let positions = [
        initial(),
        disconnected(),
        after_d3(),
        initial(),
        disconnected(),
    ];
    let sample = fixture.sample("sample", &positions, absolute_population());
    let output = fixture.path("filters");
    estimate::filters(&sample, &output, ALPHA).unwrap();

    let mut seen = HashSet::new();
    let unique: Vec<_> = positions
        .iter()
        .copied()
        .filter(|position| seen.insert(*position))
        .collect();
    let input = fixture.path("reference-input.txt");
    write_positions(&input, &unique);
    let mut current = input;
    for filter in ["con", "edge", "occupancy", "seg3more", "lp"] {
        let reference = fixture.path(&format!("reference-{filter}"));
        let status = Command::new(env!("CARGO_BIN_EXE_run_filters"))
            .arg(filter)
            .arg("--out-dir")
            .arg(&reference)
            .arg(&current)
            .status()
            .unwrap();
        assert!(status.success(), "run_filters {filter} failed");
        for suffix in ["OK.txt", "NG.txt"] {
            assert_eq!(
                fs::read(output.join(format!("{filter}_{suffix}"))).unwrap(),
                fs::read(reference.join(format!("{filter}_{suffix}"))).unwrap(),
                "{filter}_{suffix} differs from run_filters",
            );
        }
        current = reference.join(format!("{filter}_OK.txt"));
    }

    assert_eq!(
        file_names(&output),
        [
            "con_NG.txt",
            "con_OK.txt",
            "edge_NG.txt",
            "edge_OK.txt",
            "lp_NG.txt",
            "lp_OK.txt",
            "occupancy_NG.txt",
            "occupancy_OK.txt",
            "seg3more_NG.txt",
            "seg3more_OK.txt",
            "summary.json",
        ]
    );
    let summary = read_json(&output.join("summary.json"));
    assert_summary_keys(&summary, &filter_summary_keys());
    assert_eq!(summary["stage"], "filters");
    assert!(summary["from"].is_null());
    assert_eq!(summary["sample_dir"], json!(sample.canonicalize().unwrap()));
    assert_eq!(summary["counts"], json!({"OK": 0, "NG": 2, "UNKNOWN": 3}));
}

#[test]
fn duplicate_sample_rows_keep_their_weight_for_three_populations() {
    for (position, population, size, mode) in [
        (
            initial(),
            json!({"definition": "absolute_positions_fixed_discs", "discs": 4, "size": "32"}),
            32.0,
            "exact",
        ),
        (
            initial(),
            json!({"definition": "equivalence_classes_fixed_discs", "discs": 4, "size": "6"}),
            6.0,
            "equivalence",
        ),
        (
            after_d3(),
            json!({"definition": "absolute_positions_fixed_discs", "discs": 5, "size": "3840"}),
            3840.0,
            "exact",
        ),
    ] {
        let fixture = Fixture::new();
        let sample = fixture.sample("sample", &[position; 3], population.clone());
        let filters = fixture.path("filters");
        estimate::filters(&sample, &filters, ALPHA).unwrap();
        assert_eq!(
            fs::read_to_string(filters.join("lp_OK.txt")).unwrap(),
            position_text(&[position]),
        );

        let sat = fixture.path("sat");
        estimate::sat(&filters, &sat, ALPHA, SatEncoding::Ply, 2, None).unwrap();
        let summary = read_json(&sat.join("summary.json"));
        assert_summary_keys(&summary, &base_summary_keys());
        assert_eq!(summary["stage"], "sat:ply");
        assert_eq!(summary["from"], json!(filters.canonicalize().unwrap()));
        assert!(summary.get("sample_dir").is_none());
        assert_eq!(summary["population"], population);
        assert_eq!(summary["mode"], mode);
        assert_eq!(summary["with_replacement"], true);
        assert_eq!(summary["accepted_samples"], 3);
        assert_eq!(summary["counts"], json!({"OK": 3, "NG": 0, "UNKNOWN": 0}));
        assert_eq!(summary["alpha"], ALPHA);
        close(summary["wilson_interval"][0].as_f64().unwrap(), 0.75);
        close(summary["wilson_interval"][1].as_f64().unwrap(), 1.0);
        close(
            summary["reachable_interval"][0]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            0.75 * size,
        );
        close(
            summary["reachable_interval"][1]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            size,
        );
        let records = read_records(&sat.join(format!("sat_{mode}_records.csv")));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_index, 1);
        assert_eq!(records[0].goal, position);
        assert_eq!(records[0].goal_match.as_str(), mode);
        records[0].verify().unwrap();
    }
}

#[test]
fn black_turn_d3_is_exact_ng_but_equivalence_ok_in_sat() {
    let cases = [
        (
            json!({"definition": "absolute_positions_fixed_discs", "discs": 5, "size": "3840"}),
            "exact",
            "NG",
        ),
        (
            json!({"definition": "equivalence_classes_fixed_discs", "discs": 5, "size": "264"}),
            "equivalence",
            "OK",
        ),
    ];
    for (population, mode, expected) in cases {
        let fixture = Fixture::new();
        let filters = fixture.filters("sample", "filters", &[class_d3()], population);
        let sat = fixture.path("sat");
        estimate::sat(&filters, &sat, ALPHA, SatEncoding::Ply, 1, None).unwrap();
        assert_eq!(
            fs::read_to_string(sat.join(format!("sat_{mode}_{expected}.txt"))).unwrap(),
            position_text(&[class_d3()]),
        );
        let records = read_records(&sat.join(format!("sat_{mode}_records.csv")));
        if expected == "OK" {
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].goal, class_d3());
            assert_eq!(records[0].goal_match, GoalMatch::EquivalenceClass);
            records[0].verify().unwrap();
        } else {
            assert!(records.is_empty());
        }
    }
}

#[test]
fn filter_ng_rows_skip_sat_and_sat_indices_use_lp_input_lines() {
    let fixture = Fixture::new();
    let sample = fixture.sample(
        "sample",
        &[disconnected(), disconnected(), after_d3(), after_d3()],
        json!({"definition": "absolute_positions_fixed_discs", "discs": 5, "size": "3840"}),
    );
    let filters = fixture.path("filters");
    estimate::filters(&sample, &filters, ALPHA).unwrap();
    assert_eq!(
        fs::read_to_string(filters.join("con_NG.txt")).unwrap(),
        position_text(&[disconnected()]),
    );
    assert_eq!(
        fs::read_to_string(filters.join("lp_OK.txt")).unwrap(),
        position_text(&[after_d3()]),
    );

    let sat = fixture.path("sat");
    estimate::sat(&filters, &sat, ALPHA, SatEncoding::Ply, 1, None).unwrap();
    let records = read_records(&sat.join("sat_exact_records.csv"));
    assert_eq!(records, [d3_record(1)]);
    records[0].verify().unwrap();
    assert_eq!(
        read_json(&sat.join("summary.json"))["counts"],
        json!({"OK": 2, "NG": 2, "UNKNOWN": 0}),
    );
}

#[test]
fn all_filter_ng_rows_produce_empty_sat_outputs() {
    let fixture = Fixture::new();
    let filters = fixture.filters(
        "sample",
        "filters",
        &[disconnected(); 2],
        absolute_population(),
    );
    assert!(fs::read(filters.join("lp_OK.txt")).unwrap().is_empty());
    let sat = fixture.path("sat");
    estimate::sat(&filters, &sat, ALPHA, SatEncoding::Ply, 1, None).unwrap();
    for suffix in ["OK.txt", "NG.txt", "UNKNOWN.txt", "records.csv"] {
        assert!(fs::read(sat.join(format!("sat_exact_{suffix}")))
            .unwrap()
            .is_empty());
    }
    let summary = read_json(&sat.join("summary.json"));
    assert_eq!(summary["counts"], json!({"OK": 0, "NG": 2, "UNKNOWN": 0}));
    close(summary["wilson_interval"][0].as_f64().unwrap(), 0.0);
    close(summary["wilson_interval"][1].as_f64().unwrap(), 1.0 / 3.0);
}

#[test]
fn sat_stage_names_for_every_encoding_are_reread() {
    for encoding in SatEncoding::ALL {
        let fixture = Fixture::new();
        let filters = fixture.filters("sample", "filters", &[initial()], absolute_population());
        let sat = fixture.path(&format!("sat-{encoding}"));
        estimate::sat(&filters, &sat, ALPHA, encoding, 1, None).unwrap();
        assert_eq!(
            read_json(&sat.join("summary.json"))["stage"],
            format!("sat:{encoding}")
        );

        let next = fixture.path("next");
        estimate::sat(&sat, &next, ALPHA, SatEncoding::Ply, 1, None).unwrap();
        assert_eq!(
            read_json(&next.join("summary.json"))["counts"],
            json!({"OK": 1, "NG": 0, "UNKNOWN": 0}),
        );
    }
}

#[test]
fn invalid_sampling_metadata_is_rejected_by_filters() {
    let variants = [
        ("nonuniform", "uniform", json!(false)),
        ("schema", "schema_version", json!(2)),
        ("method", "method", json!("random-play")),
        (
            "fixed method on whole population",
            "method",
            json!("fixed-discs"),
        ),
        (
            "generated without replacement",
            "with_replacement",
            json!(false),
        ),
        (
            "replacement is not Boolean",
            "with_replacement",
            json!("true"),
        ),
        ("negative seed", "seed", json!(-1)),
        ("seed is not an integer", "seed", json!(1.5)),
        ("wrong accepted count", "accepted_count", json!(2)),
        (
            "wrong population size",
            "population",
            json!({"definition": "absolute_positions_center_occupied", "size": "32"}),
        ),
        (
            "unknown population",
            "population",
            json!({"definition": "other", "size": "32"}),
        ),
    ];
    for (name, field, value) in variants {
        let fixture = Fixture::new();
        let sample = fixture.sample("sample", &[initial()], absolute_population());
        let path = sample.join("sampling.json");
        let mut metadata = read_json(&path);
        metadata[field] = value;
        write_json(&path, &metadata);
        assert!(
            estimate::filters(&sample, &fixture.path("filters"), ALPHA).is_err(),
            "accepted {name}",
        );
    }
}

#[test]
fn sample_rows_and_methods_must_match_the_declared_population() {
    let wrong_turn = Position {
        side_to_move: Color::White,
        ..initial()
    };
    let nonrepresentative = Position {
        board: Board::new(initial().board.opponent, initial().board.player),
        ..initial()
    };
    for (name, position, definition, method, size) in [
        (
            "wrong disc count",
            after_d3(),
            "absolute_positions_fixed_discs",
            "fixed-discs",
            "32",
        ),
        (
            "white turn class",
            wrong_turn,
            "equivalence_classes_fixed_discs",
            "fixed-discs",
            "6",
        ),
        (
            "nonminimal class representative",
            nonrepresentative,
            "equivalence_classes_fixed_discs",
            "fixed-discs",
            "6",
        ),
        (
            "uniform method on fixed population",
            initial(),
            "absolute_positions_fixed_discs",
            "uniform",
            "32",
        ),
        (
            "converted fixed population",
            initial(),
            "absolute_positions_fixed_discs",
            "raw",
            "32",
        ),
        (
            "converted class population",
            initial(),
            "equivalence_classes_fixed_discs",
            "representatives",
            "6",
        ),
    ] {
        let fixture = Fixture::new();
        let sample = fixture.sample(
            "sample",
            &[position],
            json!({"definition": definition, "discs": 4, "size": size}),
        );
        let path = sample.join("sampling.json");
        let mut metadata = read_json(&path);
        metadata["method"] = json!(method);
        let provenance = fixture.path("provenance.txt");
        fs::write(&provenance, "Uniform source sample.\n").unwrap();
        metadata["provenance"] = json!({"path": provenance});
        write_json(&path, &metadata);
        assert!(
            estimate::filters(&sample, &fixture.path("filters"), ALPHA).is_err(),
            "accepted {name}",
        );
    }
}

#[test]
fn converted_methods_cannot_describe_the_whole_class_population() {
    for method in ["raw", "representatives"] {
        let fixture = Fixture::new();
        let sample = fixture.sample("sample", &[initial()], class_population());
        let provenance = fixture.path("provenance.txt");
        fs::write(&provenance, "Uniform source sample.\n").unwrap();
        let path = sample.join("sampling.json");
        let mut metadata = read_json(&path);
        metadata["method"] = json!(method);
        metadata["provenance"] = json!({"path": provenance});
        write_json(&path, &metadata);
        assert!(estimate::filters(&sample, &fixture.path("filters"), ALPHA).is_err());
    }
}

#[test]
fn converted_absolute_samples_use_only_accepted_rows_without_hash_checks() {
    for method in ["raw", "representatives"] {
        let fixture = Fixture::new();
        let sample = fixture.sample("sample", &[initial(), after_d3()], absolute_population());
        let provenance = fixture.path("provenance.txt");
        fs::write(&provenance, "Uniform source sample without replacement.\n").unwrap();
        let path = sample.join("sampling.json");
        let mut metadata = read_json(&path);
        metadata["method"] = json!(method);
        metadata["with_replacement"] = json!(false);
        metadata["input_count"] = json!(if method == "representatives" { 7 } else { 2 });
        metadata["provenance"] = json!({"path": provenance, "sha256": "not checked"});
        write_json(&path, &metadata);
        let filters = fixture.path("filters");
        estimate::filters(&sample, &filters, ALPHA).unwrap();
        let summary = read_json(&filters.join("summary.json"));
        assert_eq!(summary["accepted_samples"], 2);
        assert_eq!(summary["counts"], json!({"OK": 0, "NG": 0, "UNKNOWN": 2}));
        assert_eq!(summary["with_replacement"], false);
    }
}

#[test]
fn limited_reverse_can_be_followed_by_sat_or_a_larger_reverse() {
    let fixture = Fixture::new();
    let filters = fixture.filters(
        "sample",
        "filters",
        &[disconnected(), initial(), after_d3()],
        absolute_population(),
    );
    let limited = fixture.path("reverse-limited");
    estimate::reverse_search(&filters, &limited, ALPHA, &limited_dfs()).unwrap();
    assert_eq!(
        fs::read_to_string(limited.join("reverse_exact_OK.txt")).unwrap(),
        position_text(&[initial()]),
    );
    assert_eq!(
        fs::read_to_string(limited.join("reverse_exact_UNKNOWN.txt")).unwrap(),
        position_text(&[after_d3()]),
    );
    assert_eq!(
        read_json(&limited.join("summary.json"))["counts"],
        json!({"OK": 1, "NG": 1, "UNKNOWN": 1}),
    );
    let saved_limited_summary = fs::read(limited.join("summary.json")).unwrap();

    let sat = fixture.path("sat-after-reverse");
    estimate::sat(&limited, &sat, ALPHA, SatEncoding::Ply, 1, None).unwrap();
    assert_eq!(
        fs::read_to_string(sat.join("sat_exact_OK.txt")).unwrap(),
        position_text(&[after_d3()]),
    );
    assert_eq!(
        read_json(&sat.join("summary.json"))["counts"],
        json!({"OK": 2, "NG": 1, "UNKNOWN": 0}),
    );

    let larger = fixture.path("reverse-after-reverse");
    estimate::reverse_search(&limited, &larger, ALPHA, &complete_dfs()).unwrap();
    assert_eq!(
        fs::read_to_string(larger.join("reverse_exact_OK.txt")).unwrap(),
        position_text(&[after_d3()]),
    );
    assert_eq!(
        read_json(&larger.join("summary.json"))["counts"],
        json!({"OK": 2, "NG": 1, "UNKNOWN": 0}),
    );
    assert_eq!(
        fs::read(limited.join("summary.json")).unwrap(),
        saved_limited_summary
    );
}

#[test]
fn saved_sat_unknowns_flow_to_reverse_and_a_different_sat_encoding() {
    let fixture = Fixture::new();
    let filters = fixture.filters(
        "sample",
        "filters",
        &[disconnected(), initial(), after_d3(), after_d3()],
        absolute_population(),
    );
    let saved = saved_sat_with_unknown(&fixture, &filters, SatEncoding::Ply);
    assert_eq!(
        fs::read_to_string(saved.join("sat_exact_UNKNOWN.txt")).unwrap(),
        position_text(&[after_d3()]),
    );
    assert_eq!(
        read_json(&saved.join("summary.json"))["counts"],
        json!({"OK": 1, "NG": 1, "UNKNOWN": 2}),
    );

    let reverse = fixture.path("reverse-after-saved-sat");
    estimate::reverse_search(&saved, &reverse, ALPHA, &complete_dfs()).unwrap();
    assert_eq!(
        fs::read_to_string(reverse.join("reverse_exact_OK.txt")).unwrap(),
        position_text(&[after_d3()]),
    );
    assert_eq!(
        read_records(&reverse.join("reverse_exact_records.csv")),
        [d3_record(1)],
    );
    assert_eq!(
        read_json(&reverse.join("summary.json"))["counts"],
        json!({"OK": 3, "NG": 1, "UNKNOWN": 0}),
    );

    let sat = fixture.path("sat-after-saved-sat");
    estimate::sat(&saved, &sat, ALPHA, SatEncoding::PlacementColumns, 1, None).unwrap();
    assert_eq!(
        fs::read_to_string(sat.join("sat_exact_OK.txt")).unwrap(),
        position_text(&[after_d3()]),
    );
    assert_eq!(
        read_records(&sat.join("sat_exact_records.csv")),
        [d3_record(1)],
    );
    let summary = read_json(&sat.join("summary.json"));
    assert_eq!(summary["stage"], "sat:placement-columns");
    assert_eq!(summary["counts"], json!({"OK": 3, "NG": 1, "UNKNOWN": 0}),);
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
        let filters = fixture.filters("sample", "filters", &positions, population);
        for strategy in small_strategies(&fixture, stem) {
            let output = fixture.path(&format!("reverse-{}", strategy.name()));
            estimate::reverse_search(&filters, &output, ALPHA, &strategy).unwrap();
            let prefix = format!("reverse_{}", mode.as_str());
            assert_eq!(
                fs::read_to_string(output.join(format!("{prefix}_OK.txt"))).unwrap(),
                position_text(&positions),
                "{} did not prove both {stem} positions",
                strategy.name(),
            );
            assert!(fs::read(output.join(format!("{prefix}_NG.txt")))
                .unwrap()
                .is_empty());
            assert!(fs::read(output.join(format!("{prefix}_UNKNOWN.txt")))
                .unwrap()
                .is_empty());
            let records = read_records(&output.join(format!("{prefix}_records.csv")));
            assert_eq!(records.len(), 2);
            assert_eq!(
                records
                    .iter()
                    .map(|record| record.goal)
                    .collect::<HashSet<_>>(),
                positions.into_iter().collect(),
            );
            assert!(records.iter().all(|record| record.goal_match == mode));
            for record in records {
                record.verify().unwrap();
            }
            let summary = read_json(&output.join("summary.json"));
            assert_summary_keys(&summary, &reverse_summary_keys());
            assert_eq!(
                summary["stage"],
                format!("reverse-search:{}", strategy.name())
            );
            assert!(summary.get("sample_dir").is_none());
            assert_eq!(summary["counts"], json!({"OK": 2, "NG": 0, "UNKNOWN": 0}));
        }
    }
}

#[test]
fn external_bfs_resumes_only_the_same_input_and_settings() {
    let fixture = Fixture::new();
    let filters = fixture.filters(
        "sample-a",
        "filters-a",
        &[initial(), after_d3()],
        absolute_population(),
    );
    let tmp_dir = fixture.path("external-tmp");
    fs::create_dir(&tmp_dir).unwrap();
    let config = ExternalBfs {
        discs: 4,
        jobs: 2,
        verbose: 0,
        block_size: 4,
        tmp_dir: tmp_dir.clone(),
        resume: false,
    };
    let first = fixture.path("external-first");
    estimate::reverse_search(
        &filters,
        &first,
        ALPHA,
        &Strategy::BfsExternal(config.clone()),
    )
    .unwrap();

    let resumed = fixture.path("external-resumed");
    let mut resume_config = config.clone();
    resume_config.resume = true;
    estimate::reverse_search(
        &filters,
        &resumed,
        ALPHA,
        &Strategy::BfsExternal(resume_config.clone()),
    )
    .unwrap();
    for suffix in ["OK.txt", "NG.txt", "UNKNOWN.txt", "records.csv"] {
        assert_eq!(
            fs::read(first.join(format!("reverse_exact_{suffix}"))).unwrap(),
            fs::read(resumed.join(format!("reverse_exact_{suffix}"))).unwrap(),
        );
    }
    assert_eq!(
        read_json(&first.join("summary.json"))["counts"],
        read_json(&resumed.join("summary.json"))["counts"],
    );

    let mut changed_settings = resume_config.clone();
    changed_settings.block_size = 5;
    assert!(estimate::reverse_search(
        &filters,
        &fixture.path("external-changed-settings"),
        ALPHA,
        &Strategy::BfsExternal(changed_settings),
    )
    .is_err());

    let other_filters = fixture.filters(
        "sample-b",
        "filters-b",
        &[class_d3()],
        absolute_population(),
    );
    assert!(estimate::reverse_search(
        &other_filters,
        &fixture.path("external-changed-input"),
        ALPHA,
        &Strategy::BfsExternal(resume_config),
    )
    .is_err());
}

#[test]
fn filter_partitions_reject_missing_reordered_duplicate_overlapping_and_extra_rows() {
    for variant in ["missing", "reordered", "duplicate", "overlap", "extra"] {
        let fixture = Fixture::new();
        let filters = fixture.filters(
            "sample",
            "filters",
            &[initial(), after_d3(), disconnected()],
            absolute_population(),
        );
        match variant {
            "missing" => write_positions(&filters.join("con_OK.txt"), &[initial()]),
            "reordered" => {
                write_positions(&filters.join("con_OK.txt"), &[after_d3(), initial()]);
            }
            "duplicate" => write_positions(
                &filters.join("con_OK.txt"),
                &[initial(), initial(), after_d3()],
            ),
            "overlap" => write_positions(&filters.join("con_NG.txt"), &[disconnected(), initial()]),
            "extra" => write_positions(&filters.join("con_NG.txt"), &[disconnected(), class_d3()]),
            _ => unreachable!(),
        }
        assert_stage_rejected(&fixture, &filters, &format!("next-{variant}"));
    }
}

#[test]
fn solver_partitions_are_unordered_sets_but_reject_invalid_membership() {
    for variant in ["missing", "duplicate", "overlap", "extra"] {
        let fixture = Fixture::new();
        let (_, reverse) = fixture.unknown_reverse();
        match variant {
            "missing" => write_positions(&reverse.join("reverse_exact_UNKNOWN.txt"), &[]),
            "duplicate" => write_positions(
                &reverse.join("reverse_exact_OK.txt"),
                &[initial(), initial()],
            ),
            "overlap" => write_positions(&reverse.join("reverse_exact_NG.txt"), &[initial()]),
            "extra" => write_positions(&reverse.join("reverse_exact_NG.txt"), &[class_d3()]),
            _ => unreachable!(),
        }
        assert_stage_rejected(&fixture, &reverse, &format!("next-{variant}"));
    }

    let fixture = Fixture::new();
    let filters = fixture.filters(
        "sample",
        "filters",
        &[initial(), after_d3()],
        absolute_population(),
    );
    let reverse = fixture.path("reverse");
    estimate::reverse_search(&filters, &reverse, ALPHA, &complete_dfs()).unwrap();
    write_positions(
        &reverse.join("reverse_exact_OK.txt"),
        &[after_d3(), initial()],
    );
    let mut records = read_records(&reverse.join("reverse_exact_records.csv"));
    records.reverse();
    write_records(&reverse.join("reverse_exact_records.csv"), &records);
    let summary_path = reverse.join("summary.json");
    let mut summary = read_json(&summary_path);
    summary["nodes"] = json!("not used while rereading");
    write_json(&summary_path, &summary);
    let next = fixture.path("next");
    estimate::reverse_search(&reverse, &next, ALPHA, &limited_dfs()).unwrap();
    assert_eq!(
        read_json(&next.join("summary.json"))["counts"],
        json!({"OK": 2, "NG": 0, "UNKNOWN": 0}),
    );
}

#[test]
fn solver_records_reject_invalid_coverage_or_contents() {
    for variant in [
        "missing",
        "duplicate",
        "misindexed",
        "noninitial",
        "wrong-mode",
        "bad-replay",
    ] {
        let fixture = Fixture::new();
        let (_, sat) = fixture.sat_ok();
        let path = sat.join("sat_exact_records.csv");
        let mut record = read_records(&path).remove(0);
        match variant {
            "missing" => write_records(&path, &[]),
            "duplicate" => write_records(&path, &[record.clone(), record]),
            "misindexed" => {
                record.input_index = 2;
                write_records(&path, &[record]);
            }
            "noninitial" => {
                record.start = after_d3();
                write_records(&path, &[record]);
            }
            "wrong-mode" => {
                record.goal_match = GoalMatch::EquivalenceClass;
                write_records(&path, &[record]);
            }
            "bad-replay" => {
                record.actions = vec![PlyAction::Play(0)];
                write_records(&path, &[record]);
            }
            _ => unreachable!(),
        }
        assert_stage_rejected(&fixture, &sat, &format!("next-{variant}"));
    }
}

#[test]
fn solver_record_goals_must_match_their_indexed_input_lines() {
    let fixture = Fixture::new();
    let filters = fixture.filters(
        "sample",
        "filters",
        &[initial(), after_d3()],
        absolute_population(),
    );
    let sat = fixture.path("sat");
    estimate::sat(&filters, &sat, ALPHA, SatEncoding::Ply, 1, None).unwrap();
    let path = sat.join("sat_exact_records.csv");
    let mut records = read_records(&path);
    let d3 = records
        .iter_mut()
        .find(|record| record.goal == after_d3())
        .unwrap();
    d3.input_index = 1;
    write_records(&path, &records);
    assert_stage_rejected(&fixture, &sat, "next-wrong-indexed-goal");
}

#[test]
fn solver_records_reject_a_valid_record_for_a_non_ok_input() {
    let fixture = Fixture::new();
    let (_, reverse) = fixture.unknown_reverse();
    let path = reverse.join("reverse_exact_records.csv");
    let mut records = read_records(&path);
    records.push(d3_record(2));
    write_records(&path, &records);
    assert_stage_rejected(&fixture, &reverse, "next-extra-record");
}

#[test]
fn rereading_revalidates_the_original_sample_files() {
    for variant in ["positions", "metadata"] {
        let fixture = Fixture::new();
        let filters = fixture.filters(
            "sample",
            "filters",
            &[initial(), after_d3()],
            absolute_population(),
        );
        match variant {
            "positions" => fs::write(fixture.path("sample/positions.txt"), "invalid\n").unwrap(),
            "metadata" => {
                let path = fixture.path("sample/sampling.json");
                let mut metadata = read_json(&path);
                metadata["uniform"] = json!(false);
                write_json(&path, &metadata);
            }
            _ => unreachable!(),
        }
        assert_stage_rejected(&fixture, &filters, &format!("next-{variant}"));
    }
}

#[test]
fn summaries_reject_unknown_stages_and_invalid_from_or_sample_dir_keys() {
    for variant in [
        "unknown-stage",
        "filter-from",
        "filter-missing-from",
        "filter-missing-sample",
        "filter-relative-sample",
    ] {
        let fixture = Fixture::new();
        let filters = fixture.filters("sample", "filters", &[initial()], absolute_population());
        let path = filters.join("summary.json");
        let mut summary = read_json(&path);
        match variant {
            "unknown-stage" => summary["stage"] = json!("reverse-search:unknown"),
            "filter-from" => summary["from"] = json!(filters.canonicalize().unwrap()),
            "filter-missing-from" => {
                summary.as_object_mut().unwrap().remove("from");
            }
            "filter-missing-sample" => {
                summary.as_object_mut().unwrap().remove("sample_dir");
            }
            "filter-relative-sample" => summary["sample_dir"] = json!("sample"),
            _ => unreachable!(),
        }
        write_json(&path, &summary);
        assert_stage_rejected(&fixture, &filters, &format!("next-{variant}"));
    }

    for variant in [
        "solver-null-from",
        "solver-missing-from",
        "solver-sample-dir",
    ] {
        let fixture = Fixture::new();
        let (_, reverse) = fixture.unknown_reverse();
        let path = reverse.join("summary.json");
        let mut summary = read_json(&path);
        match variant {
            "solver-null-from" => summary["from"] = Value::Null,
            "solver-missing-from" => {
                summary.as_object_mut().unwrap().remove("from");
            }
            "solver-sample-dir" => summary["sample_dir"] = Value::Null,
            _ => unreachable!(),
        }
        write_json(&path, &summary);
        assert_stage_rejected(&fixture, &reverse, &format!("next-{variant}"));
    }

    let fixture = Fixture::new();
    let (_, reverse) = fixture.unknown_reverse();
    let path = reverse.join("summary.json");
    let mut summary = read_json(&path);
    summary["from"] = json!("filters");
    write_json(&path, &summary);
    assert_stage_rejected(&fixture, &reverse, "next-relative-from");
}

#[test]
fn summary_rejects_an_unknown_sat_encoding() {
    for encoding in [
        "unknown",
        "ply-target",
        "placement-target",
        "placement-target-columns",
    ] {
        let fixture = Fixture::new();
        let (_, sat) = fixture.sat_ok();
        let path = sat.join("summary.json");
        let mut summary = read_json(&path);
        summary["stage"] = json!(format!("sat:{encoding}"));
        write_json(&path, &summary);

        let output = fixture.path("next");
        let error = estimate::reverse_search(&sat, &output, ALPHA, &limited_dfs()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains(&format!("unknown estimate stage: sat:{encoding}")));
        assert!(!output.exists());
    }
}

#[test]
fn stage_chains_reject_cycles_missing_summaries_and_missing_directories() {
    let fixture = Fixture::new();
    let (_, reverse) = fixture.unknown_reverse();
    let path = reverse.join("summary.json");
    let mut summary = read_json(&path);
    summary["from"] = json!(reverse.canonicalize().unwrap());
    write_json(&path, &summary);
    assert_stage_rejected(&fixture, &reverse, "next-cycle");

    let fixture = Fixture::new();
    let filters = fixture.filters("sample", "filters", &[initial()], absolute_population());
    fs::remove_file(filters.join("summary.json")).unwrap();
    assert_stage_rejected(&fixture, &filters, "next-missing-summary");

    let fixture = Fixture::new();
    let (filters, reverse) = fixture.unknown_reverse();
    fs::rename(&filters, fixture.path("moved-filters")).unwrap();
    assert_stage_rejected(&fixture, &reverse, "next-missing-directory");
}

#[test]
fn reverse_ng_updates_all_duplicate_sample_rows_without_a_record() {
    let fixture = Fixture::new();
    let filters = fixture.filters(
        "sample",
        "filters",
        &[class_d3(); 2],
        json!({"definition": "absolute_positions_fixed_discs", "discs": 5, "size": "3840"}),
    );
    let reverse = fixture.path("reverse");
    estimate::reverse_search(&filters, &reverse, ALPHA, &complete_dfs()).unwrap();
    assert_eq!(
        fs::read_to_string(reverse.join("reverse_exact_NG.txt")).unwrap(),
        position_text(&[class_d3()]),
    );
    assert!(fs::read(reverse.join("reverse_exact_records.csv"))
        .unwrap()
        .is_empty());
    assert_eq!(
        read_json(&reverse.join("summary.json"))["counts"],
        json!({"OK": 0, "NG": 2, "UNKNOWN": 0}),
    );
}

#[test]
fn reverse_summary_has_positive_nodes_for_searched_ok_and_ng_without_search_csv() {
    let fixture = Fixture::new();
    let filters = fixture.filters(
        "sample",
        "filters",
        &[after_d3(), class_d3()],
        absolute_population(),
    );
    let reverse = fixture.path("reverse");
    estimate::reverse_search(&filters, &reverse, ALPHA, &complete_dfs()).unwrap();
    assert_eq!(
        fs::read_to_string(reverse.join("reverse_exact_OK.txt")).unwrap(),
        position_text(&[after_d3()]),
    );
    assert_eq!(
        fs::read_to_string(reverse.join("reverse_exact_NG.txt")).unwrap(),
        position_text(&[class_d3()]),
    );
    assert!(!file_names(&reverse)
        .iter()
        .any(|name| name.ends_with("_search.csv")));
    let summary = read_json(&reverse.join("summary.json"));
    assert_summary_keys(&summary, &reverse_summary_keys());
    assert_eq!(summary["stage"], "reverse-search:dfs");
    assert_eq!(summary["from"], json!(filters.canonicalize().unwrap()));
    assert!(summary.get("sample_dir").is_none());
    assert_eq!(summary["counts"], json!({"OK": 1, "NG": 1, "UNKNOWN": 0}));
    let nodes = summary["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert!(nodes.iter().all(|nodes| nodes.as_u64().unwrap() > 0));
}

#[test]
fn all_stages_require_new_output_directories() {
    let fixture = Fixture::new();
    let sample = fixture.sample("sample", &[initial()], absolute_population());
    let filters = fixture.path("filters");
    estimate::filters(&sample, &filters, ALPHA).unwrap();
    let output = fixture.path("existing");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("keep.txt"), "keep").unwrap();

    assert!(estimate::filters(&sample, &output, ALPHA).is_err());
    assert!(estimate::sat(&filters, &output, ALPHA, SatEncoding::Ply, 1, None).is_err());
    assert!(estimate::reverse_search(&filters, &output, ALPHA, &limited_dfs()).is_err());
    assert_eq!(fs::read_to_string(output.join("keep.txt")).unwrap(), "keep");
    assert_eq!(file_names(&output), ["keep.txt"]);
}

#[test]
fn invalid_controls_and_empty_samples_fail() {
    let fixture = Fixture::new();
    let sample = fixture.sample("sample", &[initial()], absolute_population());
    let filters = fixture.path("filters");
    estimate::filters(&sample, &filters, ALPHA).unwrap();

    for (index, alpha) in [0.0, 1.0, f64::NAN].into_iter().enumerate() {
        assert!(estimate::filters(
            &sample,
            &fixture.path(&format!("invalid-filter-alpha-{index}")),
            alpha,
        )
        .is_err());
        assert!(estimate::sat(
            &filters,
            &fixture.path(&format!("invalid-sat-alpha-{index}")),
            alpha,
            SatEncoding::Ply,
            1,
            None,
        )
        .is_err());
        assert!(estimate::reverse_search(
            &filters,
            &fixture.path(&format!("invalid-reverse-alpha-{index}")),
            alpha,
            &limited_dfs(),
        )
        .is_err());
    }
    assert!(estimate::sat(
        &filters,
        &fixture.path("parallel-zero"),
        ALPHA,
        SatEncoding::Ply,
        0,
        None,
    )
    .is_err());
    assert!(estimate::sat(
        &filters,
        &fixture.path("timeout-zero"),
        ALPHA,
        SatEncoding::Ply,
        1,
        Some(Duration::ZERO),
    )
    .is_err());

    let empty = fixture.sample("empty-sample", &[], absolute_population());
    assert!(estimate::filters(&empty, &fixture.path("empty-filters"), ALPHA).is_err());
}
