//! CLI acceptance cases from docs/plans/layer-sat-rebuild.md: classifications
//! retain the query, records replay independently, and invalid input preserves outputs.

use othello_complexity_rs::io::{format_position, parse_position};
use othello_complexity_rs::othello::{Board, BoardSymmetry, Color, Position};
use othello_complexity_rs::record::{GoalMatch, PlyAction, ReachabilityRecord};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "layer-sat-cli-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn layer_sat() -> Command {
    Command::new(env!("CARGO_BIN_EXE_layer_sat"))
}

fn verify_record() -> Command {
    Command::new(env!("CARGO_BIN_EXE_verify_record"))
}

fn expect_success(output: Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read_records(path: &Path) -> Vec<ReachabilityRecord> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| ReachabilityRecord::from_tsv(line).unwrap())
        .collect()
}

fn occurrences(lines: impl IntoIterator<Item = String>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for line in lines {
        *counts.entry(line).or_insert(0) += 1;
    }
    counts
}

// D3 is a legal Black opening. It flips D4; the next player is White at E5.
fn after_d3() -> Position {
    Position {
        board: Board::new(1 << 36, (1 << 19) | (1 << 27) | (1 << 28) | (1 << 35)),
        side_to_move: Color::White,
    }
}

#[test]
fn parallel_classifications_and_records_preserve_duplicate_input_numbers_and_turns() {
    let directory = TestDirectory::new("duplicates");
    let initial = Position::initial();
    let other_turn = Position {
        side_to_move: Color::White,
        ..initial
    };
    let goals = [initial, other_turn, after_d3(), initial];
    let input = directory.path().join("goals.txt");
    fs::write(&input, goals.map(format_position).join("\n\n") + "\n").unwrap();
    expect_success(
        layer_sat()
            .args(["--from-initial", "--parallel-goals", "2", "--goal-file"])
            .arg(&input)
            .arg("--out-dir")
            .arg(directory.path())
            .output()
            .unwrap(),
    );

    let ok_path = directory.path().join("layer_sat_exact_OK.txt");
    let ok = fs::read_to_string(&ok_path).unwrap();
    let expected = occurrences([initial, after_d3(), initial].map(format_position));
    assert_eq!(occurrences(ok.lines().map(str::to_owned)), expected);
    assert_eq!(
        fs::read_to_string(directory.path().join("layer_sat_exact_NG.txt")).unwrap(),
        format!("{}\n", format_position(other_turn))
    );
    assert!(
        fs::read_to_string(directory.path().join("layer_sat_exact_UNKNOWN.txt"))
            .unwrap()
            .is_empty()
    );

    let records_path = directory.path().join("layer_sat_exact_records.tsv");
    let records = read_records(&records_path);
    let mut indices: Vec<_> = records.iter().map(|record| record.input_index).collect();
    indices.sort_unstable();
    assert_eq!(indices, vec![1, 3, 4]);
    assert_eq!(
        occurrences(records.iter().map(|record| format_position(record.goal))),
        expected
    );
    for record in &records {
        assert_eq!(record.start, initial);
        assert_eq!(record.goal, goals[record.input_index - 1]);
        assert_eq!(record.goal_match, GoalMatch::Exact);
        assert_eq!(record.symmetry, BoardSymmetry::Identity);
        assert_eq!(record.verify().unwrap().last(), Some(&record.goal));
        if record.goal == initial {
            assert!(record.actions.is_empty());
        } else {
            assert_eq!(record.actions, vec![PlyAction::Play(19)]);
        }
    }
    // The last empty TSV field is part of the empty-record contract.
    for line in fs::read_to_string(&records_path).unwrap().lines() {
        assert_eq!(line.split('\t').count(), 6);
        if line.starts_with("1\t") || line.starts_with("4\t") {
            assert!(line.ends_with('\t'));
        }
    }
    expect_success(
        verify_record()
            .arg("--input-file")
            .arg(&records_path)
            .output()
            .unwrap(),
    );

    // Reusing a classification file preserves the same query and multiplicities.
    let second = directory.path().join("second");
    expect_success(
        layer_sat()
            .args(["--from-initial", "--goal-file"])
            .arg(ok_path)
            .arg("--out-dir")
            .arg(&second)
            .output()
            .unwrap(),
    );
    let second_ok = fs::read_to_string(second.join("layer_sat_exact_OK.txt")).unwrap();
    assert_eq!(occurrences(second_ok.lines().map(str::to_owned)), expected);
}

#[test]
fn symmetry_output_keeps_the_requested_goal_and_the_selected_transform() {
    let directory = TestDirectory::new("symmetry");
    let initial = Position::initial();
    let goal = BoardSymmetry::ALL
        .into_iter()
        .map(|s| s.apply_position(initial))
        .find(|p| *p != initial)
        .unwrap();
    let position = format_position(goal);

    expect_success(
        layer_sat()
            .args(["--from-initial", "--goal", &position, "--out-dir"])
            .arg(directory.path())
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("layer_sat_exact_NG.txt")).unwrap(),
        format!("{position}\n")
    );
    assert!(
        fs::read_to_string(directory.path().join("layer_sat_exact_records.tsv"))
            .unwrap()
            .is_empty()
    );

    expect_success(
        layer_sat()
            .args([
                "--from-initial",
                "--goal",
                &position,
                "--check-symmetry",
                "--out-dir",
            ])
            .arg(directory.path())
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("layer_sat_symmetry_OK.txt")).unwrap(),
        format!("{position}\n")
    );
    let records_path = directory.path().join("layer_sat_symmetry_records.tsv");
    let records = read_records(&records_path);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.goal, goal);
    assert_eq!(record.goal_match, GoalMatch::SymmetryOrbit);
    assert_eq!(record.symmetry.apply_position(goal), initial);
    assert!(record.actions.is_empty());
    expect_success(
        verify_record()
            .arg("--input-file")
            .arg(records_path)
            .output()
            .unwrap(),
    );
    expect_success(
        verify_record()
            .args([
                "--from-initial",
                "--goal",
                &position,
                "--record",
                "",
                "--check-symmetry",
            ])
            .output()
            .unwrap(),
    );
}

#[test]
fn a_pass_only_sat_record_is_saved_and_verified() {
    let directory = TestDirectory::new("pass");
    let start =
        parse_position("OXX-OOOOOXXXXXXOOXOOOXXOOOXOXOXXOXOXOOXXOOOXXXOXOOXOOOXXOOOOOOXX w")
            .unwrap();
    let goal = Position {
        board: start.board.swapped(),
        side_to_move: Color::Black,
    };
    expect_success(
        layer_sat()
            .args([
                "--start",
                &format_position(start),
                "--goal",
                &format_position(goal),
                "--out-dir",
            ])
            .arg(directory.path())
            .output()
            .unwrap(),
    );
    let path = directory.path().join("layer_sat_exact_records.tsv");
    let records = read_records(&path);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].actions, vec![PlyAction::Pass]);
    assert_eq!(records[0].verify().unwrap(), vec![start, goal]);
    expect_success(
        verify_record()
            .arg("--input-file")
            .arg(path)
            .output()
            .unwrap(),
    );
}

#[test]
fn invalid_input_is_rejected_before_existing_outputs_are_opened() {
    let directory = TestDirectory::new("invalid");
    let output_names = [
        "layer_sat_exact_OK.txt",
        "layer_sat_exact_NG.txt",
        "layer_sat_exact_UNKNOWN.txt",
        "layer_sat_exact_records.tsv",
    ];
    for name in output_names {
        fs::write(directory.path().join(name), name).unwrap();
    }
    let input = directory.path().join("goals.txt");
    let initial = format_position(Position::initial());
    let no_turn = Position::initial().board.to_string();
    for contents in [
        format!("{initial}\n\n{no_turn}\n"),
        format!("{initial}\n\n{initial} extra\n"),
        String::new(),
    ] {
        fs::write(&input, &contents).unwrap();
        let output = layer_sat()
            .args(["--from-initial", "--goal-file"])
            .arg(&input)
            .arg("--out-dir")
            .arg(directory.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        if !contents.is_empty() {
            assert!(String::from_utf8_lossy(&output.stderr).contains("line 3"));
        }
        for name in output_names {
            assert_eq!(
                fs::read_to_string(directory.path().join(name)).unwrap(),
                name
            );
        }
    }

    let unused_output = directory.path().join("must-not-be-created");
    let invalid_start = "---------------------------------------------------------------- b";
    let output = layer_sat()
        .args(["--start", invalid_start, "--goal", &initial, "--out-dir"])
        .arg(&unused_output)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!unused_output.exists());
}

#[test]
fn cnf_only_run_creates_no_classifications_or_records_and_preserves_duplicate_candidates() {
    let directory = TestDirectory::new("cnf");
    let initial = format_position(Position::initial());
    let input = directory.path().join("goals.txt");
    fs::write(&input, format!("{initial}\n{initial}\n")).unwrap();
    let output_dir = directory.path().join("output");
    expect_success(
        layer_sat()
            .args(["--from-initial", "--goal-file"])
            .arg(input)
            .args(["--cnf-dump-only", "--out-dir"])
            .arg(&output_dir)
            .output()
            .unwrap(),
    );
    let files: Vec<_> = fs::read_dir(&output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 2);
    for path in files {
        assert_eq!(path.extension().unwrap(), "cnf");
        assert!(fs::read_to_string(path)
            .unwrap()
            .lines()
            .any(|line| line.starts_with("p cnf ")));
    }
}

#[test]
fn file_verification_rejects_a_disallowed_or_incorrect_transform_and_wrong_turn() {
    let directory = TestDirectory::new("tampered");
    let initial = Position::initial();
    let symmetry = BoardSymmetry::ALL
        .into_iter()
        .find(|s| s.apply_position(initial) != initial)
        .unwrap();
    let mut record = ReachabilityRecord {
        input_index: 1,
        start: initial,
        goal: initial,
        goal_match: GoalMatch::Exact,
        symmetry: BoardSymmetry::Identity,
        actions: vec![],
    };
    let path = directory.path().join("records.tsv");
    let mut invalid_records = vec!["\t\t\t\t\t".to_owned(), " ".to_owned()];
    record.symmetry = symmetry;
    invalid_records.push(record.to_tsv()); // Nonidentity is forbidden even for exact queries.
    record.goal_match = GoalMatch::SymmetryOrbit;
    invalid_records.push(record.to_tsv()); // This recorded transform does not reach the endpoint.
    record.symmetry = BoardSymmetry::Identity;
    record.goal.side_to_move = Color::White;
    invalid_records.push(record.to_tsv());
    record.goal = initial;
    record.actions = vec![PlyAction::Pass];
    invalid_records.push(record.to_tsv()); // A legal opening cannot pass.
    record.actions.clear();
    let text = record.to_tsv();
    let mut fields: Vec<_> = text.split('\t').collect();
    fields[4] = "unknown_transform";
    invalid_records.push(fields.join("\t"));
    for text in invalid_records {
        fs::write(&path, format!("\n{text}\n")).unwrap();
        let output = verify_record()
            .arg("--input-file")
            .arg(&path)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("line 2"));
    }
}
