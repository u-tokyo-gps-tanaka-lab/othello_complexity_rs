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
