use clap::Parser;
use std::fs;
use std::io;
use std::path::{Component, Path};

use othello_complexity_rs::lib::bfs_search::{
    retrospective_search_bfs_par, retrospective_search_bfs_par_resume, Cfg,
};
use othello_complexity_rs::lib::reverse_common::{
    ensure_outputs, read_boards, validate_board, LeafCache,
};

fn split_path_components(p: &Path) -> Vec<String> {
    p.components()
        .map(|c| match c {
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir
            | Component::Normal(_) => c.as_os_str().to_string_lossy().into_owned(),
        })
        .collect()
}

fn run(cfg: &Cfg) -> io::Result<()> {
    println!("cfg={:?}", cfg);

    fs::create_dir_all(&cfg.out_dir)?;
    fs::create_dir_all(&cfg.tmp_dir)?;
    let mut outputs = ensure_outputs(&cfg.out_dir)?;
    println!("info: writing outputs under '{}'", cfg.out_dir.display());

    let discs = cfg.discs as i32;
    let leaf_cache = LeafCache::new(discs);
    println!(
        "info: discs = {}: internal = {}, leaf = {}",
        cfg.discs,
        leaf_cache.searched_count(),
        leaf_cache.leaf_count()
    );

    let input_path = &cfg.input;
    if cfg.resume {
        let parts = split_path_components(input_path);
        let last = parts
            .last()
            .expect("input path must have trailing component");
        println!("last={}", last);
        let sp_under: Vec<&str> = last.split_terminator('_').collect();
        let sp_dot: Vec<&str> = sp_under[1].split_terminator('.').collect();
        let num_disc: i32 = sp_dot[0].parse().unwrap();
        retrospective_search_bfs_par_resume(cfg, num_disc, discs, leaf_cache.leaf())?;
        outputs.flush()?;
        return Ok(());
    }

    let boards = read_boards(input_path)?;
    let total_input = boards.len();
    println!(
        "info: read {} board(s) from '{}'.",
        total_input,
        input_path.display()
    );

    for board in boards {
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        let stat = retrospective_search_bfs_par(cfg, &board, discs, leaf_cache.leaf())?;
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
