use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "reverse-to-initial-cli-{}-{}",
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

#[test]
fn dfs_writes_only_classification_and_record_files_and_reports_nodes() {
    let directory = TestDirectory::new();
    let input = directory.path().join("input.txt");
    let out_dir = directory.path().join("out");
    let position = "-------------------O-------OO------OX--------------------------- w";
    fs::write(&input, format!("{position}\n")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_reverse_to_initial"))
        .arg("dfs")
        .arg(&input)
        .arg("--out-dir")
        .arg(&out_dir)
        .args(["--discs", "4", "--mode", "exact"])
        .env_remove("DISCS")
        .env_remove("MAX_NODES")
        .env_remove("TABLE_SIZE")
        .env_remove("RAYON_THREADS")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut files = fs::read_dir(&out_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    files.sort_unstable();
    assert_eq!(
        files,
        [
            "reverse_exact_NG.txt",
            "reverse_exact_OK.txt",
            "reverse_exact_UNKNOWN.txt",
            "reverse_exact_records.csv",
        ]
    );
    assert_eq!(
        fs::read_to_string(out_dir.join("reverse_exact_OK.txt")).unwrap(),
        format!("{position}\n")
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let nodes = stdout
        .lines()
        .find_map(|line| line.strip_prefix("input 1: OK nodes="))
        .expect("stdout must report the first input as reachable")
        .parse::<usize>()
        .unwrap();
    assert!(nodes > 0);
}
