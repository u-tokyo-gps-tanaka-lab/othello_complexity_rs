use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;

use othello_complexity_rs::lib::io::parse_file_to_boards;
use othello_complexity_rs::lib::othello::Board;
use othello_complexity_rs::lib::search::{init_rayon, retrospective_search_parallel, search, SearchResult};

const CENTER_MASK: u64 = 0x0000_0018_1800_0000u64; // 4 center squares

fn run() -> io::Result<()> {
    let mut input: Option<String> = None;
    let mut out_dir_s: Option<String> = None;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--out-dir" => {
                out_dir_s = Some(args.next().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "missing value for --out-dir")
                })?);
            }
            _ => {
                if arg.starts_with('-') {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("unknown flag: {}", arg),
                    ));
                } else if input.is_none() {
                    input = Some(arg);
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("unexpected extra argument: {}", arg),
                    ));
                }
            }
        }
    }

    let input_path = input.unwrap_or_else(|| "board.txt".to_string());
    let boards = parse_file_to_boards(&input_path)?;
    let total_input = boards.len();
    println!("info: read {} board(s) from '{}'.", total_input, input_path);

    // Resolve output directory
    let out_dir = out_dir_s
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result"));

    // Ensure output directory exists
    fs::create_dir_all(&out_dir)?;
    // Output files
    let mut ok = File::create(out_dir.join("reverse_OK.txt"))?;
    let mut ng = File::create(out_dir.join("reverse_NG.txt"))?;
    let mut unknown = File::create(out_dir.join("reverse_UNKNOWN.txt"))?;
    println!("info: writing outputs under '{}'", out_dir.display());

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
    //let mut retrospective_searched: HashSet<[u64; 2]> = HashSet::new();
    //let mut retrospective_searched: Btable = Btable::new(0x100000000, 0x10000);
    //let mut retroflips: Vec<[u64; 10_000]> = vec![];

    // Node limit for reverse search (unique nodes). Configurable via MAX_NODES
    let node_limit: usize = env::var("MAX_NODES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000);
    println!("info: MAX_NODES = {}", node_limit);
    // Node limit for reverse search (unique nodes). Configurable via TABLE_SIZE
    let table_limit: usize = env::var("TABLE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_00_000);
    println!("info: TABLE_SIZE = {}", table_limit);
    init_rayon(Some(60));
    //let visited = DashSet::new();

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
        //retrospective_searched.clear();
        // retroflips is grown lazily inside the function as needed
        let stat = retrospective_search_parallel(
            &b,
            /*from_pass=*/ false,
            discs,
            &leafnode,
            node_limit,
            table_limit,
        );
        match stat {
            SearchResult::Found => {
                writeln!(ok, "{}", line)?;
            }
            SearchResult::NotFound => {
                writeln!(ng, "{}", line)?;
            }
            SearchResult::Unknown => {
                writeln!(unknown, "{}", line)?;
            }
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
