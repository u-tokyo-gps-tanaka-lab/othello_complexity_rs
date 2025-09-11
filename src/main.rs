use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;

use othello_complexity_rs::lib::io::parse_file_to_boards;
use othello_complexity_rs::lib::solve_kissat::is_sat_ok;

fn process_file(path: &str) -> io::Result<()> {
    let boards = parse_file_to_boards(path)?;

    // Ensure project-root `result` directory exists and write outputs there
    let result_dir: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result");
    fs::create_dir_all(&result_dir)?;
    let mut okfile = File::create(result_dir.join("sat_OK.txt"))?;
    let mut ngfile = File::create(result_dir.join("sat_NG.txt"))?;

    for (index, b) in boards.iter().enumerate() {
        let line = b.to_string();
        match is_sat_ok(index, &line) {
            Ok(true) => {
                println!("{}", line);
                writeln!(okfile, "{}", line)?;
            }
            Ok(false) => {
                writeln!(ngfile, "{}", line)?;
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
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            if let Err(e) = process_file(arg) {
                eprintln!("Error: {}", e);
            }
        }
        println!("argv[{}] : {}", i, arg);
    }
}
