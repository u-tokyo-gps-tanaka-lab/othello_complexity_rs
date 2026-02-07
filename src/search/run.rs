use std::fs;
use std::io;
use std::path::Path;

use crate::io::{ensure_outputs, parse_file_to_boards};
use crate::othello::validate_board;

use crate::search::{
    dfs::retrospective_search,
    external_bfs::{
        parallel_retrospective_bfs, parallel_retrospective_bfs_resume, read_target_board,
        unblocked_retrospective_bfs, Cfg as BfsCfg,
    },
    forward::make_fwd_table,
    inmemory_bfs::parallel_inmemory_retrospective_bfs,
    move_ordering::retrospective_search_move_ordering,
    parallel_dfs::{init_rayon, retrospective_search_parallel},
    parallel_gbfs::parallel_retrospective_greedy_best_first_search,
    transposition::{Btable, LeafCache},
};

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
pub fn run_dfs_move_ordering(
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
pub fn run_parallel_dfs(
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
pub fn run_parallel_gbfs(
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

    let rayon_threads = match rayon_threads {
        Some(n) if n > 0 => n,
        Some(_) => 1,
        None => std::thread::available_parallelism()
            .map(|n| n.get().min(64))
            .unwrap_or(1),
    };
    for board in boards {
        let leaf = make_fwd_table(&[board.player, board.opponent], discs);
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        let result = parallel_retrospective_greedy_best_first_search(
            &board,
            discs,
            &leaf,
            node_limit,
            use_lp,
            rayon_threads,
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

    for board in boards {
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        let leaf = make_fwd_table(&[board.player, board.opponent], discs);
        let stat = unblocked_retrospective_bfs(cfg, &board, discs, &leaf)?;
        outputs.write_result(stat, &line)?;
        outputs.flush()?;
    }

    outputs.flush()
}

/// parallel bfs
pub fn run_parallel_bfs(cfg: &BfsCfg) -> io::Result<()> {
    println!("cfg={:?}", cfg);

    fs::create_dir_all(&cfg.out_dir)?;
    fs::create_dir_all(&cfg.tmp_dir)?;
    let mut outputs = ensure_outputs(&cfg.out_dir)?;
    println!("info: writing outputs under '{}'", cfg.out_dir.display());

    let discs = cfg.discs as i32;

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
        let target = read_target_board(&cfg.tmp_dir)?;
        let leaf = make_fwd_table(&target, discs);
        parallel_retrospective_bfs_resume(cfg, num_disc, discs, &leaf)?;
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

        let leaf = make_fwd_table(&[board.player, board.opponent], discs);
        let stat = parallel_retrospective_bfs(cfg, &board, discs, &leaf)?;
        outputs.write_result(stat, &line)?;
        outputs.flush()?;
    }

    outputs.flush()
}

/// In-memory parallel BFS
pub fn run_parallel_inmemory_bfs(
    input: &Path,
    out_dir: &Path,
    discs: i32,
    node_limit: usize,
    use_lp: bool,
    use_occupancy_cache: bool,
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

    for board in boards {
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        let leaf = make_fwd_table(&[board.player, board.opponent], discs);
        println!("got leaf nodes");

        let result = parallel_inmemory_retrospective_bfs(
            &board,
            discs,
            &leaf,
            node_limit,
            use_lp,
            use_occupancy_cache,
        );
        outputs.write_result(result, &line)?;
        outputs.flush()?;
    }

    outputs.flush()
}
