use clap::Parser;
use std::path::{Path, PathBuf, Component};
use std::io::{self, Write};
use std::fs::{self, File};
use std::collections::HashSet;
use othello_complexity_rs::lib::io::parse_file_to_boards;
use othello_complexity_rs::lib::othello::{Board, CENTER_MASK};
use othello_complexity_rs::lib::search::{search, SearchResult};
use othello_complexity_rs::lib::bfs_search::{retrospective_search_bfs_par, retrospective_search_bfs_par_resume, Cfg};


fn split_path_components(p: &Path) -> Vec<String> {
    p.components()
        .map(|c| match c {
            // ルートやプレフィックスもそのまま文字列化したい場合
            Component::Prefix(_) | Component::RootDir
            | Component::CurDir   | Component::ParentDir
            | Component::Normal(_) => c.as_os_str().to_string_lossy().into_owned(),
        })
        .collect()
}
fn run(cfg: &Cfg) -> io::Result<()> {
    println!("cfg={:?}", cfg);

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
    let discs = cfg.discs;
    search(&initial, &mut searched, &mut leafnode, discs as i32);
    println!(
        "info: discs = {}: internal = {}, leaf = {}",
        discs,
        searched.len(),
        leafnode.len()
    );
    let input_path = &cfg.input;
    if cfg.resume {
        let parts = split_path_components(input_path);
        println!("last={}", parts[parts.len() - 1]);
        let sp_under:Vec<&str> = parts[parts.len() - 1].split_terminator('_').collect();
        let sp_dot:Vec<&str> = sp_under[1].split_terminator('.').collect();
        let num_disc: i32 = sp_dot[0].parse().unwrap();
        retrospective_search_bfs_par_resume(cfg, num_disc as i32, discs as i32, &leafnode)?;
        return Ok(());
    }
    let boards = parse_file_to_boards(&input_path.to_str().unwrap())?;

    let total_input = boards.len();
    println!("info: read {} board(s) from '{}'.", total_input, input_path.display());

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
        let stat = retrospective_search_bfs_par(
            &cfg,
            &b,
            discs as i32,
            &leafnode,
        )?;
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