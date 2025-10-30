use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use othello_complexity_rs::io::parse_file_to_boards;
use othello_complexity_rs::othello::Board;
use othello_complexity_rs::prunings::check_occupancy::check_occupancy_with_string;

fn is_board_ok(_index: usize, board: &Board) -> io::Result<(bool, String)> {
    let o = board.player | board.opponent;
    Ok(check_occupancy_with_string(o))
}

fn process_file(path: &str, out_dir: &Path) -> io::Result<()> {
    let boards = parse_file_to_boards(path)?;

    // Ensure output directory exists and write outputs there
    fs::create_dir_all(out_dir)?;
    let mut okfile = File::create(out_dir.join("occupancy_OK.txt"))?;
    let mut ngfile = File::create(out_dir.join("occupancy_NG.txt"))?;
    let mut okfile_explainable = File::create(out_dir.join("occupancy_OK_explainable.txt"))?;
    let mut ngfile_explainable = File::create(out_dir.join("occupancy_NG_explainable.txt"))?;

    for (index, b) in boards.iter().enumerate() {
        let line = b.to_string();
        match is_board_ok(index, b) {
            Ok((true, res)) => {
                writeln!(okfile, "{}", line)?;
                writeln!(okfile_explainable, "{}", res)?;
            }
            Ok((false, res)) => {
                writeln!(ngfile, "{}", line)?;
                writeln!(ngfile_explainable, "{}", res)?;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    }

    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // Parse options: -o DIR | --out-dir DIR | --out-dir=DIR | -o=DIR
    let mut out_dir: Option<PathBuf> = None;
    let mut inputs: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-o" | "--out-dir" => {
                if i + 1 >= args.len() {
                    eprintln!("Option {} requires a directory path", arg);
                    std::process::exit(2);
                }
                out_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
                continue;
            }
            _ => {
                if let Some(rest) = arg.strip_prefix("--out-dir=") {
                    out_dir = Some(PathBuf::from(rest));
                    i += 1;
                    continue;
                }
                if let Some(rest) = arg.strip_prefix("-o=") {
                    out_dir = Some(PathBuf::from(rest));
                    i += 1;
                    continue;
                }
                inputs.push(arg.clone());
                i += 1;
            }
        }
    }

    let out_dir_path: PathBuf =
        out_dir.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result"));

    // Keep previous simple argv debug print
    for (i, arg) in args.iter().enumerate() {
        println!("argv[{}] : {}", i, arg);
    }

    for input in inputs {
        if let Err(e) = process_file(&input, &out_dir_path) {
            eprintln!("Error: {}", e);
        }
    }
}
