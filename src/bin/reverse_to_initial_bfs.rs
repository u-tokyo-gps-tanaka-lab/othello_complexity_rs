use clap::Parser;
use othello_complexity_rs::lib::bfs_search::{retrospective_search_bfs, Cfg};
use othello_complexity_rs::lib::io::parse_file_to_boards;
use othello_complexity_rs::lib::othello::{Board, CENTER_MASK};
use othello_complexity_rs::lib::search::{search, SearchResult};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;

fn run(cfg: &Cfg) -> io::Result<()> {
    println!("cfg={:?}", cfg);
    let input_path = &cfg.input;
    let boards = parse_file_to_boards(&input_path.to_str().unwrap())?;
    let discs = cfg.discs;
    let total_input = boards.len();
    println!(
        "info: read {} board(s) from '{}'.",
        total_input,
        input_path.display()
    );

    // Resolve output directory
    let out_dir = &cfg.out_dir;

    // Ensure output directory exists
    fs::create_dir_all(&out_dir)?;
    let tmp_dir = &cfg.tmp_dir;
    fs::create_dir_all(&tmp_dir)?;
    // Output files
    let mut ok = File::create(out_dir.join("reverse_OK.txt"))?;
    let mut ng = File::create(out_dir.join("reverse_NG.txt"))?;
    let mut unknown = File::create(out_dir.join("reverse_UNKNOWN.txt"))?;
    println!("info: writing outputs under '{}'", out_dir.display());
    let mut searched: HashSet<[u64; 2]> = HashSet::new();
    let mut leafnode: HashSet<[u64; 2]> = HashSet::new();
    let initial = Board::initial();
    search(&initial, &mut searched, &mut leafnode, discs as i32);
    println!(
        "info: discs = {}: internal = {}, leaf = {}",
        discs,
        searched.len(),
        leafnode.len()
    );
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
        let stat = retrospective_search_bfs(&cfg, &b, discs as i32, &leafnode)?;
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
    let cfg: Cfg = Cfg::parse();
    if let Err(e) = run(&cfg) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
