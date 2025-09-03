use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};

use othello_complexity_rs::lib::othello::Board;
use othello_complexity_rs::lib::search::{retrospective_search, search};

const CENTER_MASK: u64 = 0x0000_0018_1800_0000u64; // 4 center squares

fn parse_line_to_board(line: &str) -> Option<Board> {
    let mut player: u64 = 0;
    let mut opponent: u64 = 0;
    let mut idx = 0u32;
    for c in line.chars() {
        match c {
            'X' => {
                if idx >= 64 {
                    return None;
                }
                player |= 1_u64 << idx;
                idx += 1;
            }
            'O' => {
                if idx >= 64 {
                    return None;
                }
                opponent |= 1_u64 << idx;
                idx += 1;
            }
            '-' => {
                if idx >= 64 {
                    return None;
                }
                idx += 1;
            }
            _ => (),
        }
    }
    if idx == 64 {
        Some(Board::new(player, opponent))
    } else {
        None
    }
}

fn parse_file_to_boards(path: &str) -> io::Result<Vec<Board>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut boards: Vec<Board> = Vec::new();

    // 1) per-line 64-char records
    for line in reader.lines() {
        let l = line?;
        let filtered: String = l
            .chars()
            .filter(|&c| c == 'X' || c == 'O' || c == '-')
            .collect();
        if filtered.len() == 64 {
            if let Some(b) = parse_line_to_board(&filtered) {
                boards.push(b);
            }
        }
    }

    if !boards.is_empty() {
        return Ok(boards);
    }

    // 2) fallback: whole file aggregated as a single 8x8
    let file = File::open(path)?;
    let mut all = String::new();
    BufReader::new(file).read_to_string(&mut all)?;
    let filtered: String = all
        .chars()
        .filter(|&c| c == 'X' || c == 'O' || c == '-')
        .collect();
    if filtered.len() == 64 {
        if let Some(b) = parse_line_to_board(&filtered) {
            return Ok(vec![b]);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "failed to parse any 64-cell X/O/- board(s)",
    ))
}

fn run() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let input_path = if args.len() >= 2 {
        &args[1]
    } else {
        "board.txt"
    };

    let boards = parse_file_to_boards(input_path)?;

    // Ensure project-root `result` directory exists and write outputs there
    let result_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result");
    fs::create_dir_all(&result_dir)?;
    // Output files (UNKNOWN will remain empty under Plan A)
    let mut ok = File::create(result_dir.join("reverse_OK.txt"))?;
    let mut ng = File::create(result_dir.join("reverse_NG.txt"))?;
    let _unknown = File::create(result_dir.join("reverse_UNKNOWN.txt"))?; // kept for compatibility

    // Threshold for leaf collection
    let discs: i32 = env::var("DISCS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    // Precompute leaf nodes by forward search from the initial position
    let mut searched: HashSet<[u64; 2]> = HashSet::new();
    let mut leafnode: HashSet<[u64; 2]> = HashSet::new();
    let initial = Board::initial();
    search(&initial, &mut searched, &mut leafnode, discs);
    println!(
        "info: discs = {}: internal = {}, leaf = {}",
        discs,
        searched.len(),
        leafnode.len()
    );

    // Buffers reused for each retrospective search
    let mut retrospective_searched: HashSet<[u64; 2]> = HashSet::new();
    let mut retroflips: Vec<[u64; 10_000]> = vec![];

    for b in boards {
        let line = b.to_string();

        // quick sanity checks to avoid panics inside library helpers
        if (b.player & b.opponent) != 0 {
            writeln!(ng, "{}", line)?;
            continue;
        }
        let occupied = b.player | b.opponent;
        if (occupied & CENTER_MASK) != CENTER_MASK {
            writeln!(ng, "{}", line)?;
            continue;
        }

        // fresh visited set per board
        retrospective_searched.clear();
        // retroflips is grown lazily inside the function as needed

        if retrospective_search(
            &b,
            false,
            discs,
            &leafnode,
            &mut retrospective_searched,
            &mut retroflips,
        ) {
            writeln!(ok, "{}", line)?;
        } else {
            writeln!(ng, "{}", line)?;
        }
    }

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
