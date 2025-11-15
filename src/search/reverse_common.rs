use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::io::{ensure_outputs, parse_file_to_boards};
use crate::othello::validate_board;

use crate::search::{
    bfs::{
        retrospective_search_bfs, retrospective_search_bfs_par,
        retrospective_search_bfs_par_resume, Cfg as BfsCfg,
    },
    core::{retrospective_search, Btable},
    leaf_cache::LeafCache,
    move_ordering::retrospective_search_move_ordering,
    parallel_dfs::{init_rayon, retrospective_search_parallel},
    parallel_gbfs::parallel_retrospective_greedy_best_first_search,
    search_fwd_par::make_fwd_table,
};

pub fn default_input_path() -> PathBuf {
    PathBuf::from("board.txt")
}

pub fn default_out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result")
}

pub fn read_env_with_default<T>(key: &str, default: T) -> T
where
    T: FromStr,
{
    env::var(key)
        .ok()
        .and_then(|s| s.parse::<T>().ok())
        .unwrap_or(default)
}

/// pure dfs
pub fn run_dfs(input: &Path, out_dir: &Path, discs: i32, node_limit: usize) -> io::Result<()> {
    let boards = parse_file_to_boards(&input.to_string_lossy())?;
    let total_input = boards.len();
    println!(
        "info: read {} board(s) from '{}'.",
        total_input,
        input.display()
    );

    let mut outputs = ensure_outputs(out_dir)?;
    println!("info: writing outputs under '{}'", out_dir.display());

    let leaf_cache = LeafCache::new(discs);
    println!(
        "info: discs = {}: internal = {}, leaf = {}",
        discs,
        leaf_cache.searched_count(),
        leaf_cache.leaf_count()
    );

    let mut retrospective_searched: Btable = Btable::new(0x100000000, 0x10000);
    let mut retroflips: Vec<[u64; 10_000]> = vec![];

    for board in boards {
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        retrospective_searched.clear();
        let mut node_count: usize = 0;

        let result = retrospective_search(
            &board,
            false,
            discs,
            leaf_cache.leaf(),
            &mut retrospective_searched,
            &mut retroflips,
            &mut node_count,
            node_limit,
        );
        outputs.write_result(result, &line)?;
        outputs.flush()?;
    }

    outputs.flush()
}

/// dfs + move ordering
pub fn run_move_ordering(
    input: &Path,
    out_dir: &Path,
    discs: i32,
    node_limit: usize,
) -> io::Result<()> {
    let boards = parse_file_to_boards(&input.to_string_lossy())?;
    let total_input = boards.len();
    println!(
        "info: read {} board(s) from '{}'.",
        total_input,
        input.display()
    );

    let mut outputs = ensure_outputs(out_dir)?;
    println!("info: writing outputs under '{}'", out_dir.display());

    let leaf_cache = LeafCache::new(discs);
    println!(
        "info: discs = {}: internal = {}, leaf = {}",
        discs,
        leaf_cache.searched_count(),
        leaf_cache.leaf_count()
    );

    let mut retrospective_searched: Btable = Btable::new(0x100000000, 0x10000);
    let mut retroflips: Vec<[u64; 10_000]> = vec![];

    for board in boards {
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        retrospective_searched.clear();
        let mut node_count: usize = 0;

        let result = retrospective_search_move_ordering(
            &board,
            false,
            discs,
            leaf_cache.leaf(),
            &mut retrospective_searched,
            &mut retroflips,
            &mut node_count,
            node_limit,
        );
        outputs.write_result(result, &line)?;
        outputs.flush()?;
    }

    outputs.flush()
}

/// parallel dfs
pub fn run_parallel(
    input: &Path,
    out_dir: &Path,
    discs: i32,
    node_limit: usize,
    table_limit: usize,
    rayon_threads: Option<usize>,
) -> io::Result<()> {
    let boards = parse_file_to_boards(&input.to_string_lossy())?;
    let total_input = boards.len();
    println!(
        "info: read {} board(s) from '{}'.",
        total_input,
        input.display()
    );

    let mut outputs = ensure_outputs(out_dir)?;
    println!("info: writing outputs under '{}'", out_dir.display());

    let leaf_cache = LeafCache::new(discs);
    println!(
        "info: discs = {}: internal = {}, leaf = {}",
        discs,
        leaf_cache.searched_count(),
        leaf_cache.leaf_count()
    );

    init_rayon(rayon_threads);

    for board in boards {
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        let result = retrospective_search_parallel(
            &board,
            false,
            discs,
            leaf_cache.leaf(),
            node_limit,
            table_limit,
        );
        outputs.write_result(result, &line)?;
        outputs.flush()?;
    }

    outputs.flush()
}

/// parallel greedy best first search + priority queue (skiplist)
pub fn run_parallel1(
    input: &Path,
    out_dir: &Path,
    discs: i32,
    node_limit: usize,
    use_lp: bool,
    rayon_threads: Option<usize>,
) -> io::Result<()> {
    let boards = parse_file_to_boards(&input.to_string_lossy())?;
    let total_input = boards.len();
    println!(
        "info: read {} board(s) from '{}'.",
        total_input,
        input.display()
    );

    let mut outputs = ensure_outputs(out_dir)?;
    println!("info: writing outputs under '{}'", out_dir.display());

    //let leaf_cache = LeafCache::new(discs);
    //println!(
    //    "info: discs = {}: internal = {}, leaf = {}",
    //    discs,
    //    leaf_cache.searched_count(),
    //    leaf_cache.leaf_count()
    //);

    init_rayon(rayon_threads);

    for board in boards {
        let leaf = make_fwd_table(&[board.player, board.opponent], discs);
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        let result = parallel_retrospective_greedy_best_first_search(
            &board, discs, &leaf, node_limit, use_lp,
        );
        outputs.write_result(result, &line)?;
        outputs.flush()?;
    }

    outputs.flush()
}

/// sequential bfs
pub fn run_bfs(cfg: &BfsCfg) -> io::Result<()> {
    println!("cfg={:?}", cfg);
    let boards = parse_file_to_boards(&cfg.input.to_string_lossy())?;
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
        outputs.flush()?;
    }

    outputs.flush()
}

/// parallel bfs
pub fn run_bfs_par(cfg: &BfsCfg) -> io::Result<()> {
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

    if cfg.resume {
        let input_path = &cfg.input;
        let parts: Vec<String> = input_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let last = parts
            .last()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "input path is empty"))?;
        println!("last={}", last);
        let sp_under: Vec<&str> = last.split_terminator('_').collect();
        if sp_under.len() < 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("failed to parse resume filename: {}", last),
            ));
        }
        let sp_dot: Vec<&str> = sp_under[1].split_terminator('.').collect();
        let num_disc: i32 = sp_dot[0].parse().map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("failed to parse disc count from {}: {e}", last),
            )
        })?;
        retrospective_search_bfs_par_resume(cfg, num_disc, discs, leaf_cache.leaf())?;
        return outputs.flush();
    }

    let boards = parse_file_to_boards(&cfg.input.to_string_lossy())?;
    let total_input = boards.len();
    println!(
        "info: read {} board(s) from '{}'.",
        total_input,
        cfg.input.display()
    );

    for board in boards {
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        let stat = retrospective_search_bfs_par(cfg, &board, discs, leaf_cache.leaf())?;
        outputs.write_result(stat, &line)?;
        outputs.flush()?;
    }

    outputs.flush()
}
