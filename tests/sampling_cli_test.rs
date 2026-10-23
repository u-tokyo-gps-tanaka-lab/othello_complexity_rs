use othello_complexity_rs::othello::Position;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "sampling-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
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

fn assert_success(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn sampling_record(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path.join("sampling.json")).unwrap()).unwrap()
}

#[test]
fn convert_boards_defaults_seed_to_42() {
    let directory = TestDirectory::new();
    let input = directory.path().join("input.txt");
    let provenance = directory.path().join("provenance.txt");
    let default = directory.path().join("default");
    let explicit = directory.path().join("explicit");
    fs::write(
        &input,
        format!("{}\n", Position::initial().board.to_string()),
    )
    .unwrap();
    fs::write(&provenance, "test provenance\n").unwrap();

    let mut default_command = Command::new(env!("CARGO_BIN_EXE_convert_boards"));
    default_command
        .arg("raw")
        .arg("--input")
        .arg(&input)
        .arg("--out-dir")
        .arg(&default)
        .args(["--source-sampling", "with-replacement"])
        .arg("--provenance")
        .arg(&provenance);
    assert_success(&mut default_command);

    let mut explicit_command = Command::new(env!("CARGO_BIN_EXE_convert_boards"));
    explicit_command
        .arg("raw")
        .arg("--input")
        .arg(&input)
        .arg("--out-dir")
        .arg(&explicit)
        .args(["--seed", "42"])
        .args(["--source-sampling", "with-replacement"])
        .arg("--provenance")
        .arg(&provenance);
    assert_success(&mut explicit_command);

    assert_eq!(sampling_record(&default)["seed"], 42);
    assert_eq!(
        fs::read(default.join("positions.txt")).unwrap(),
        fs::read(explicit.join("positions.txt")).unwrap()
    );
}

#[test]
fn gen_random_boards_defaults_seed_to_42() {
    let directory = TestDirectory::new();
    let default = directory.path().join("default");
    let explicit = directory.path().join("explicit");

    let mut default_command = Command::new(env!("CARGO_BIN_EXE_gen_random_boards"));
    default_command
        .args(["--mode", "exact", "--count", "1", "--out-dir"])
        .arg(&default);
    assert_success(&mut default_command);

    let mut explicit_command = Command::new(env!("CARGO_BIN_EXE_gen_random_boards"));
    explicit_command
        .args(["--mode", "exact", "--count", "1", "--out-dir"])
        .arg(&explicit)
        .args(["--seed", "42"]);
    assert_success(&mut explicit_command);

    assert_eq!(sampling_record(&default)["seed"], 42);
    assert_eq!(
        fs::read(default.join("positions.txt")).unwrap(),
        fs::read(explicit.join("positions.txt")).unwrap()
    );
}
