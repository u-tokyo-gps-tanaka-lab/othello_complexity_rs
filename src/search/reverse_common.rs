use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::io::parse_file_to_boards;
use crate::othello::{Board, CENTER_MASK};
use crate::search::bfs_search::{
    retrospective_search_bfs, retrospective_search_bfs_par, retrospective_search_bfs_par_resume,
    Cfg as BfsCfg,
};
use crate::search::move_ordering::retrospective_search_move_ordering;
use crate::search::par_search::{init_rayon, retrospective_search_parallel};
use crate::search::par_search1::retrospective_search_parallel1;
use crate::search::search::{retrospective_search, search, Btable, SearchResult};
use crate::search::search_fwd_par::make_fwd_table;

pub fn default_input_path() -> PathBuf {
    PathBuf::from("board.txt")
}

pub fn default_out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result")
}

pub fn read_boards(path: &Path) -> io::Result<Vec<Board>> {
    parse_file_to_boards(&path.to_string_lossy())
}

pub fn ensure_outputs(out_dir: &Path) -> io::Result<ReverseOutputs> {
    fs::create_dir_all(out_dir)?;
    ReverseOutputs::create(out_dir)
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

pub struct LeafCache {
    searched: HashSet<[u64; 2]>,
    leaf: HashSet<[u64; 2]>,
}

impl LeafCache {
    pub fn new(discs: i32) -> Self {
        let mut searched: HashSet<[u64; 2]> = HashSet::new();
        let mut leafnode: HashSet<[u64; 2]> = HashSet::new();
        let initial = Board::initial();
        search(&initial, &mut searched, &mut leafnode, discs);
        for i in 4..9 {
            let mut ans = vec![];
            for s in &searched {
                if (s[0] | s[1]).count_ones() == i {
                    ans.push(s);
                }
            }
            println!("i={}, ans.len()={}", i, ans.len());
            ans.sort();
            for j in 0..ans.len() {
                println!("{}", Board::new(ans[j][0], ans[j][1]).to_string());
            }
        }
        LeafCache {
            searched,
            leaf: leafnode,
        }
    }

    pub fn searched_count(&self) -> usize {
        self.searched.len()
    }

    pub fn leaf_count(&self) -> usize {
        self.leaf.len()
    }

    pub fn leaf(&self) -> &HashSet<[u64; 2]> {
        &self.leaf
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardValidation {
    Overlap,
    MissingCenter,
}

pub fn validate_board(board: &Board) -> Result<(), BoardValidation> {
    if (board.player & board.opponent) != 0 {
        return Err(BoardValidation::Overlap);
    }
    let occupied = board.player | board.opponent;
    if (occupied & CENTER_MASK) != CENTER_MASK {
        return Err(BoardValidation::MissingCenter);
    }
    Ok(())
}

pub struct ReverseOutputs {
    pub ok: io::BufWriter<File>,
    pub ng: io::BufWriter<File>,
    pub unknown: io::BufWriter<File>,
}

impl ReverseOutputs {
    fn create(out_dir: &Path) -> io::Result<Self> {
        let ok = io::BufWriter::new(File::create(out_dir.join("reverse_OK.txt"))?);
        let ng = io::BufWriter::new(File::create(out_dir.join("reverse_NG.txt"))?);
        let unknown = io::BufWriter::new(File::create(out_dir.join("reverse_UNKNOWN.txt"))?);
        Ok(ReverseOutputs { ok, ng, unknown })
    }

    pub fn write_result(&mut self, result: SearchResult, line: &str) -> io::Result<()> {
        match result {
            SearchResult::Found => writeln!(self.ok, "{}", line),
            SearchResult::NotFound => writeln!(self.ng, "{}", line),
            SearchResult::Unknown => writeln!(self.unknown, "{}", line),
        }
    }

    pub fn write_invalid(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.ng, "{}", line)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.ok.flush()?;
        self.ng.flush()?;
        self.unknown.flush()?;
        Ok(())
    }
}

pub fn run_dfs(input: &Path, out_dir: &Path, discs: i32, node_limit: usize) -> io::Result<()> {
    let boards = read_boards(input)?;
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

pub fn run_move_ordering(
    input: &Path,
    out_dir: &Path,
    discs: i32,
    node_limit: usize,
) -> io::Result<()> {
    let boards = read_boards(input)?;
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

pub fn run_parallel(
    input: &Path,
    out_dir: &Path,
    discs: i32,
    node_limit: usize,
    table_limit: usize,
    rayon_threads: Option<usize>,
) -> io::Result<()> {
    let boards = read_boards(input)?;
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

pub fn run_parallel1(
    input: &Path,
    out_dir: &Path,
    discs: i32,
    node_limit: usize,
    table_limit: usize,
    rayon_threads: Option<usize>,
) -> io::Result<()> {
    let boards = read_boards(input)?;
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

        let result = retrospective_search_parallel1(&board, discs, &leaf, node_limit, table_limit);
        outputs.write_result(result, &line)?;
        outputs.flush()?;
    }

    outputs.flush()
}

pub fn run_bfs(cfg: &BfsCfg) -> io::Result<()> {
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
        outputs.flush()?;
    }

    outputs.flush()
}

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

    let boards = read_boards(&cfg.input)?;
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
