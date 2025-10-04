use clap::Parser;
use std::fs;
use std::io;

use othello_complexity_rs::lib::bfs_search::{retrospective_search_bfs, Cfg};
use othello_complexity_rs::lib::reverse_common::{
    ensure_outputs, read_boards, validate_board, LeafCache,
};

fn run(cfg: &Cfg) -> io::Result<()> {
    println!("cfg={:?}", cfg);
    let boards = read_boards(&cfg.input)?;
    let discs = cfg.discs as i32;
    let total_input = boards.len();
    println!(
        "info: read {} board(s) from '{}'.",
        total_input,
        cfg.input.display()
    );

    fs::create_dir_all(&cfg.out_dir)?;
    fs::create_dir_all(&cfg.tmp_dir)?;

    let mut outputs = ensure_outputs(&cfg.out_dir)?;
    println!("info: writing outputs under '{}'", cfg.out_dir.display());

    let leaf_cache = LeafCache::new(discs);
    println!(
        "info: discs = {}: internal = {}, leaf = {}",
        cfg.discs,
        leaf_cache.searched_count(),
        leaf_cache.leaf_count()
    );

    for board in boards {
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        let stat = retrospective_search_bfs(cfg, &board, discs, leaf_cache.leaf())?;
        outputs.write_result(stat, &line)?;
    }

    outputs.flush()
}

fn main() {
    let cfg: Cfg = Cfg::parse();
    if let Err(e) = run(&cfg) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
