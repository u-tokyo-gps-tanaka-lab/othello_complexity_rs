use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{Board, BoardSymmetry, Color, Position};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "filter-cli-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn check(filter: &str, input: &Path, output: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_check"))
        .arg(filter)
        .arg(input)
        .arg("--out-dir")
        .arg(output)
        .output()
        .unwrap()
}

fn succeeds(output: Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn classifications_preserve_turn_orientation_and_multiplicity() {
    let directory = TestDirectory::new("preservation");
    let initial = Position::initial();
    let after_c4 = Position {
        board: Board::new(1 << 36, (1 << 26) | (1 << 27) | (1 << 28) | (1 << 35)),
        side_to_move: Color::White,
    };
    let positions = [
        initial,
        Position {
            side_to_move: Color::White,
            ..initial
        },
        BoardSymmetry::MirrorHorizontal.apply_position(initial),
        after_c4,
        initial,
    ];
    let text = positions.map(format_position).join("\n") + "\n";
    let input = directory.0.join("input.txt");
    fs::write(&input, &text).unwrap();
    for (command, prefix) in [
        ("con", "con"),
        ("edge", "edge"),
        ("occupancy", "occupancy"),
        ("seg3more", "seg3more"),
        ("lp", "lp"),
        ("flip-sat", "flip_sat"),
    ] {
        let output = directory.0.join(prefix);
        succeeds(check(command, &input, &output));
        assert_eq!(
            fs::read_to_string(output.join(format!("{prefix}_OK.txt"))).unwrap(),
            text
        );
        assert!(fs::read(output.join(format!("{prefix}_NG.txt")))
            .unwrap()
            .is_empty());
    }
    let edge_output = directory.0.join("edge-standalone");
    succeeds(
        Command::new(env!("CARGO_BIN_EXE_edge_pattern"))
            .arg(&input)
            .arg("--out-dir")
            .arg(&edge_output)
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read_to_string(edge_output.join("edge_OK.txt")).unwrap(),
        text
    );
}

#[test]
fn rejected_rows_also_keep_their_turn_order_and_duplicates() {
    let directory = TestDirectory::new("rejected");
    let initial = Position::initial();
    let disconnected = Position {
        board: Board::new(initial.board.player | 1, initial.board.opponent),
        side_to_move: Color::White,
    };
    let other_turn = Position {
        side_to_move: Color::Black,
        ..disconnected
    };
    let rows = [disconnected, initial, other_turn, disconnected];
    let input = directory.0.join("input.txt");
    fs::write(&input, rows.map(format_position).join("\n") + "\n").unwrap();
    let output = directory.0.join("output");
    succeeds(check("con", &input, &output));
    assert_eq!(
        fs::read_to_string(output.join("con_OK.txt")).unwrap(),
        format!("{}\n", format_position(initial))
    );
    assert_eq!(
        fs::read_to_string(output.join("con_NG.txt")).unwrap(),
        [disconnected, other_turn, disconnected]
            .map(format_position)
            .join("\n")
            + "\n"
    );
}

#[test]
fn invalid_position_rows_are_not_classified() {
    let directory = TestDirectory::new("invalid");
    let input = directory.0.join("input.txt");
    let valid = format_position(Position::initial());
    for invalid in [valid[..64].to_owned(), String::new(), format!("{valid} b")] {
        fs::write(&input, format!("{valid}\n{invalid}\n")).unwrap();
        let output = directory.0.join("output");
        let result = check("con", &input, &output);
        assert!(String::from_utf8_lossy(&result.stderr).contains("line 2"));
        assert!(!output.exists());
    }
}

#[test]
fn direct_board_argument_requires_the_absolute_turn() {
    let initial = format_position(Position::initial());
    for turn in ['b', 'w'] {
        let position = format!("{} {turn}", &initial[..64]);
        let output = Command::new(env!("CARGO_BIN_EXE_check"))
            .args(["con", "--board", &position])
            .output()
            .unwrap();
        assert_eq!(output.stdout, b"OK\n");
        succeeds(output);
    }
    assert!(!Command::new(env!("CARGO_BIN_EXE_check"))
        .args(["con", "--board", &initial[..64]])
        .output()
        .unwrap()
        .status
        .success());
}

#[test]
fn sat_file_matching_distinguishes_absolute_turns() {
    let directory = TestDirectory::new("sat-matching");
    let black = format_position(Position::initial());
    let white = format!("{} w", &black[..64]);
    let input = directory.0.join("input.txt");
    let sat = directory.0.join("sat.txt");
    let output = directory.0.join("output");
    fs::write(&input, format!("{white}\n{black}\n{white}\n{black}\n")).unwrap();
    fs::write(&sat, format!("{black}\n")).unwrap();
    succeeds(
        Command::new(env!("CARGO_BIN_EXE_fix_sat_file"))
            .arg(&input)
            .arg("--sat-file")
            .arg(&sat)
            .arg("--out-dir")
            .arg(&output)
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read_to_string(output.join("sat_OK.txt")).unwrap(),
        format!("{black}\n{black}\n")
    );
    assert_eq!(
        fs::read_to_string(output.join("sat_NG.txt")).unwrap(),
        format!("{white}\n{white}\n")
    );
}

#[test]
fn diagnostic_masks_are_explicit_and_are_not_position_inputs() {
    let directory = TestDirectory::new("diagnostic");
    let initial = format_position(Position::initial());
    let mask: String = initial[..64]
        .chars()
        .map(|cell| if cell == '-' { '-' } else { 'G' })
        .collect();
    let run = |arguments: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_vis_occupancy_steps"))
            .args(arguments)
            .current_dir(&directory.0)
            .output()
            .unwrap()
    };
    succeeds(run(&[&initial]));
    succeeds(run(&["--mask", &mask]));
    assert!(!run(&[&mask]).status.success());
    assert!(!run(&[&initial[..64]]).status.success());
    assert!(!run(&["--mask", &initial[..64]]).status.success());
}
