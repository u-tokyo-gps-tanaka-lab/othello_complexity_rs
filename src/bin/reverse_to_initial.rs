use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;

use othello_complexity_rs::lib::io::parse_file_to_boards_generic;
use othello_complexity_rs::lib::othello::{Board, Geometry, Standard6x6, Standard8x8};
use othello_complexity_rs::lib::search::{retrospective_search, search, Btable, SearchResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoardSize {
    Size8,
    Size6,
}

impl BoardSize {
    fn suffix(self) -> &'static str {
        match self {
            BoardSize::Size8 => "",
            BoardSize::Size6 => "_6x6",
        }
    }
}

fn parse_args() -> (BoardSize, Option<String>, Option<String>) {
    let mut input: Option<String> = None;
    let mut out_dir: Option<String> = None;
    let mut size = BoardSize::Size8;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--out-dir" => {
                out_dir = Some(args.next().unwrap_or_else(|| {
                    eprintln!("missing value for --out-dir");
                    process::exit(1);
                }));
            }
            "-s" | "--size" => {
                size = match args
                    .next()
                    .unwrap_or_else(|| {
                        eprintln!("--size には 6 または 8 を指定してください");
                        process::exit(1);
                    })
                    .as_str()
                {
                    "6" | "6x6" => BoardSize::Size6,
                    "8" | "8x8" => BoardSize::Size8,
                    other => {
                        eprintln!("未知のサイズ指定: {} (6 または 8)", other);
                        process::exit(1);
                    }
                };
            }
            "-h" | "--help" => {
                println!("Usage: reverse_to_initial [--size <6|8>] [-o DIR] [board.txt]");
                process::exit(0);
            }
            _ => {
                if arg.starts_with('-') {
                    eprintln!("unknown flag: {}", arg);
                    process::exit(1);
                } else if input.is_none() {
                    input = Some(arg);
                } else {
                    eprintln!("unexpected extra argument: {}", arg);
                    process::exit(1);
                }
            }
        }
    }

    (size, input, out_dir)
}

fn run_with_geometry<G: Geometry>(
    input_path: &str,
    out_dir: &Path,
    discs: i32,
    suffix: &str,
) -> io::Result<()> {
    let boards = parse_file_to_boards_generic::<G>(input_path)?;
    let total_input = boards.len();
    println!("info: read {} board(s) from '{}'.", total_input, input_path);

    fs::create_dir_all(out_dir)?;
    let mut ok = File::create(out_dir.join(format!("reverse_OK{}.txt", suffix)))?;
    let mut ng = File::create(out_dir.join(format!("reverse_NG{}.txt", suffix)))?;
    let mut unknown = File::create(out_dir.join(format!("reverse_UNKNOWN{}.txt", suffix)))?;
    println!("info: writing outputs under '{}'", out_dir.display());

    let mut searched: HashSet<[u64; 2]> = HashSet::new();
    let mut leafnode: HashSet<[u64; 2]> = HashSet::new();
    let initial: Board<G> = Board::initial();
    search::<G>(&initial, &mut searched, &mut leafnode, discs);
    println!(
        "info: discs = {}: internal = {}, leaf = {}",
        discs,
        searched.len(),
        leafnode.len()
    );

    let mut retrospective_searched: Btable = Btable::new(0x10000000, 0x10000);
    let mut retroflips: Vec<[u64; 10_000]> = vec![];

    let node_limit: usize = env::var("MAX_NODES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000);
    println!("info: MAX_NODES = {}", node_limit);
    let mut total_nodes: usize = 0;
    let center_mask = G::center_mask();

    for b in boards {
        let line = b.to_string();
        if (b.player & b.opponent) != 0 {
            writeln!(ng, "{}", line)?;
            continue;
        }
        let occupied = b.player | b.opponent;
        if (occupied & center_mask) != center_mask {
            writeln!(ng, "{}", line)?;
            continue;
        }

        retrospective_searched.clear();
        let mut node_count: usize = 0;

        let result = retrospective_search::<G>(
            &b,
            false,
            discs,
            &leafnode,
            &mut retrospective_searched,
            &mut retroflips,
            &mut node_count,
            node_limit,
        );

        total_nodes += node_count;

        match result {
            SearchResult::Found => writeln!(ok, "{}", line)?,
            SearchResult::NotFound => writeln!(ng, "{}", line)?,
            SearchResult::Unknown => writeln!(unknown, "{}", line)?,
        }
    }

    println!("info: total reverse nodes visited = {}", total_nodes);

    Ok(())
}

fn run() -> io::Result<()> {
    let (board_size, input_opt, out_dir_opt) = parse_args();

    let input_path = input_opt.unwrap_or_else(|| "board.txt".to_string());
    let out_dir = out_dir_opt
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result"));

    let discs: i32 = env::var("DISCS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    match board_size {
        BoardSize::Size8 => run_with_geometry::<Standard8x8>(
            &input_path,
            &out_dir,
            discs,
            BoardSize::Size8.suffix(),
        ),
        BoardSize::Size6 => run_with_geometry::<Standard6x6>(
            &input_path,
            &out_dir,
            discs,
            BoardSize::Size6.suffix(),
        ),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
