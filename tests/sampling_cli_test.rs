#![allow(dead_code)]
mod support;

use othello_complexity_rs::io::{format_position, parse_file_to_positions};
use othello_complexity_rs::othello::Color;
use othello_complexity_rs::record::PlyAction;
use othello_complexity_rs::sampling::position_from_rank;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use support::ArrayPosition;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "absolute-sampling-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn random_samples_write_absolute_positions_and_preserve_the_requested_disc_count() {
    let directory = TestDir::new();
    for noncentral in [0, 1, 60] {
        let output = Command::new(env!("CARGO_BIN_EXE_gen_random_boards"))
            .args(["--gen-count", "32", "--stone-count"])
            .arg(noncentral.to_string())
            .arg("--out-dir")
            .arg(&directory.0)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let path = directory.0.join(format!("result_n{noncentral}_c32.txt"));
        let positions = parse_file_to_positions(path.to_str().unwrap()).unwrap();
        assert_eq!(positions.len(), 32);
        if noncentral != 0 {
            assert!(positions
                .iter()
                .all(|p| p.board.popcount() == noncentral + 4));
        }
    }
}

#[test]
fn random_play_endpoints_retain_the_turn_determined_by_legal_actions_and_passes() {
    let directory = TestDir::new();
    for placements in [1, 60] {
        let output = Command::new(env!("CARGO_BIN_EXE_random_play"))
            .args(["--gen-count", "12", "--stone-count"])
            .arg(placements.to_string())
            .arg("--out-dir")
            .arg(&directory.0)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let prefix = directory.0.join(format!("random_play_n{placements}_c12"));
        let goals =
            parse_file_to_positions(prefix.with_extension("txt").to_str().unwrap()).unwrap();
        let records = fs::read_to_string(
            directory
                .0
                .join(format!("random_play_n{placements}_c12_record.txt")),
        )
        .unwrap();
        assert_eq!(goals.len(), 12);
        assert_eq!(records.lines().count(), 12);
        for (goal, record) in goals.iter().zip(records.lines()) {
            assert_eq!(record.len() % 2, 0);
            let mut position = ArrayPosition::initial();
            for coordinate in record.as_bytes().chunks_exact(2) {
                if position.moves().is_empty() {
                    position = position.apply(PlyAction::Pass).unwrap();
                }
                let square =
                    usize::from(coordinate[0] - b'a') + 8 * usize::from(coordinate[1] - b'1');
                position = position.apply(PlyAction::Play(square)).unwrap();
            }
            assert_eq!(*goal, position.position());
            if placements == 1 {
                assert_eq!(goal.side_to_move, Color::White);
            }
        }
    }
}

#[test]
fn integer_input_preserves_both_turns_and_duplicates_through_standard_streams() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rnd_to_txt"))
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"0\n1\n0\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let expected = [0, 1, 0]
        .map(|rank| format_position(position_from_rank(rank).unwrap()))
        .join("\n")
        + "\n";
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
}

// Append to tests/sampling_cli_test.rs; its crate root must declare mod support once.
mod conversion_contracts {
    use super::support::{ArrayPosition, ALL_SYMMETRIES};
    use othello_complexity_rs::io::{format_position, parse_file_to_positions};
    use othello_complexity_rs::othello::{Board, Color, Position, CENTER_MASK};
    use othello_complexity_rs::sampling::sha256;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "absolute-conversion-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn run(binary: &str, args: &[&str], directory: &Path) -> Output {
        Command::new(binary)
            .args(args)
            .arg("--out-dir")
            .arg(directory)
            .output()
            .unwrap()
    }
    fn success(output: Output) {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn assert_record(directory: &Path) -> serde_json::Value {
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("sampling.json")).unwrap()).unwrap();
        assert_eq!(
            record["output"]["sha256"],
            sha256(&fs::read(directory.join("positions.txt")).unwrap())
        );
        assert_eq!(record["schema_version"], 1);
        record
    }

    #[test]
    fn conversions_record_every_input_occurrence_and_are_reproducible() {
        let tmp = TestDir::new();
        let source = tmp.path("source.txt");
        fs::write(
            &source,
            format!("{0}\n{0}\n", Position::initial().board.to_string()),
        )
        .unwrap();
        let provenance = tmp.path("provenance.txt");
        fs::write(
            &provenance,
            "Test fixture: uniform input with replacement, retained representatives.\n",
        )
        .unwrap();
        let binary = env!("CARGO_BIN_EXE_convert_boards");
        for name in ["one", "two"] {
            success(run(
                binary,
                &[
                    "raw",
                    "--input",
                    source.to_str().unwrap(),
                    "--seed",
                    "11",
                    "--source-sampling",
                    "with-replacement",
                    "--provenance",
                    provenance.to_str().unwrap(),
                ],
                &tmp.path(name),
            ));
        }
        assert_eq!(
            fs::read(tmp.path("one/positions.txt")).unwrap(),
            fs::read(tmp.path("two/positions.txt")).unwrap()
        );
        assert_eq!(
            fs::read(tmp.path("one/mapping.jsonl")).unwrap(),
            fs::read(tmp.path("two/mapping.jsonl")).unwrap()
        );
        let record = assert_record(&tmp.path("one"));
        assert_eq!(
            record["input"]["sha256"],
            sha256(&fs::read(&source).unwrap())
        );
        let mapping = fs::read_to_string(tmp.path("one/mapping.jsonl")).unwrap();
        for (index, line) in mapping.lines().enumerate() {
            let row: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(row["input_line"], index + 1);
            assert_eq!(row["output_line"], index + 1);
        }
        success(run(
            binary,
            &[
                "fixed-turn",
                "--input",
                source.to_str().unwrap(),
                "--turn",
                "w",
            ],
            &tmp.path("fixed"),
        ));
        assert_eq!(assert_record(&tmp.path("fixed"))["uniform"], false);
        assert!(fs::read_to_string(tmp.path("fixed/positions.txt"))
            .unwrap()
            .lines()
            .all(|line| line.ends_with(" w")));
        fs::write(&source, "invalid\n").unwrap();
        assert!(!run(
            binary,
            &[
                "fixed-turn",
                "--input",
                source.to_str().unwrap(),
                "--turn",
                "b"
            ],
            &tmp.path("invalid")
        )
        .status
        .success());
        assert!(!tmp.path("invalid").exists());
    }

    /// The four boards have respectively 1, 2, 4, and 8 distinct geometric images.
    /// Both turns therefore give m = 2, 4, 8, 16 candidates, so acceptance of each
    /// recorded draw is fixed by the m/16 contract rather than a frequency tolerance.
    #[test]
    fn representatives_resample_self_symmetries_and_record_every_input_reproducibly() {
        let tmp = TestDir::new();
        let fixtures = [
            (Board::new(CENTER_MASK, 0), 2),
            (ArrayPosition::initial().position().board, 4),
            (Board::new(CENTER_MASK | 1, 0), 8),
            (Board::new(CENTER_MASK | (1 << 1), 0), 16),
        ];
        let source = tmp.path("representatives.txt");
        let boards: Vec<_> = (0..32)
            .flat_map(|_| fixtures.map(|(board, _)| board))
            .collect();
        let contents = boards
            .iter()
            .map(Board::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&source, &contents).unwrap();
        let provenance = tmp.path("provenance.txt");
        fs::write(
            &provenance,
            "Finite symmetric fixtures for checking the specified conversion law.\n",
        )
        .unwrap();
        for name in ["first", "second"] {
            success(run(
                env!("CARGO_BIN_EXE_convert_boards"),
                &[
                    "representatives",
                    "--input",
                    source.to_str().unwrap(),
                    "--seed",
                    "47",
                    "--source-sampling",
                    "with-replacement",
                    "--provenance",
                    provenance.to_str().unwrap(),
                ],
                &tmp.path(name),
            ));
        }
        for file in ["positions.txt", "mapping.jsonl"] {
            assert_eq!(
                fs::read(tmp.path(&format!("first/{file}"))).unwrap(),
                fs::read(tmp.path(&format!("second/{file}"))).unwrap()
            );
        }
        let output =
            parse_file_to_positions(tmp.path("first/positions.txt").to_str().unwrap()).unwrap();
        let rows: Vec<serde_json::Value> = fs::read_to_string(tmp.path("first/mapping.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), boards.len());
        let mut accepted = 0;
        let mut rejected = 0;
        for (index, row) in rows.iter().enumerate() {
            let (board, expected_count) = fixtures[index % fixtures.len()];
            let oracle = ArrayPosition::from_relative(&board.to_string(), true);
            let mut candidates = Vec::new();
            for symmetry in ALL_SYMMETRIES {
                let image = oracle.transformed(symmetry);
                for position in [image, image.color_exchanged()] {
                    let candidate = position.position();
                    if !candidates
                        .iter()
                        .any(|(_, previous)| *previous == candidate)
                    {
                        candidates.push((symmetry, candidate));
                    }
                }
            }
            assert_eq!(candidates.len(), expected_count);
            assert_eq!(row["input_line"], index + 1);
            assert_eq!(row["source_board"], boards[index].to_string());
            assert_eq!(row["candidate_count"], expected_count);
            let draw = row["draw"].as_u64().unwrap() as usize;
            assert!(draw < 16);
            assert_eq!(row["accepted"], draw < expected_count);
            if draw < expected_count {
                let (symmetry, position) = candidates[draw];
                assert_eq!(row["output_line"], accepted + 1);
                assert_eq!(row["position"], format_position(position));
                assert_eq!(row["symmetry"], symmetry.as_str());
                assert_eq!(
                    row["side_to_move"],
                    if position.side_to_move == Color::Black {
                        "b"
                    } else {
                        "w"
                    }
                );
                assert_eq!(output[accepted], position);
                accepted += 1;
            } else {
                for field in ["output_line", "position", "symmetry", "side_to_move"] {
                    assert!(
                        row[field].is_null(),
                        "a rejected row has a selected {field}"
                    );
                }
                rejected += 1;
            }
        }
        assert!(accepted > 0 && rejected > 0);
        assert_eq!(accepted, output.len());
        let record = assert_record(&tmp.path("first"));
        assert_eq!(record["input_count"], boards.len());
        assert_eq!(record["accepted_count"], accepted);
        assert_eq!(record["method"], "representatives");
        assert_eq!(record["seed"], 47);
        assert_eq!(record["input"]["sha256"], sha256(contents.as_bytes()));
        assert_eq!(
            record["provenance"]["sha256"],
            sha256(&fs::read(provenance).unwrap())
        );
        assert_eq!(
            record["mapping"]["sha256"],
            sha256(&fs::read(tmp.path("first/mapping.jsonl")).unwrap())
        );
    }

    #[test]
    fn representatives_reject_a_nonrepresentative_at_its_physical_line() {
        let tmp = TestDir::new();
        let minimum = Board::new(CENTER_MASK | 1, 0);
        // H8 has a strictly larger occupied-square mask than its A1 image.
        let nonminimum = Board::new(CENTER_MASK | (1 << 63), 0);
        let source = tmp.path("source.txt");
        fs::write(
            &source,
            format!("{}\n{}\n", minimum.to_string(), nonminimum.to_string()),
        )
        .unwrap();
        let provenance = tmp.path("provenance.txt");
        fs::write(
            &provenance,
            "Deliberately invalid representative selection.\n",
        )
        .unwrap();
        let output = tmp.path("invalid");
        let result = run(
            env!("CARGO_BIN_EXE_convert_boards"),
            &[
                "representatives",
                "--input",
                source.to_str().unwrap(),
                "--seed",
                "47",
                "--source-sampling",
                "with-replacement",
                "--provenance",
                provenance.to_str().unwrap(),
            ],
            &output,
        );
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("line 2"));
        assert!(error.contains("representative"));
        assert!(!output.exists());
    }
}
