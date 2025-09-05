use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::thread;

use othello_complexity_rs::lib::othello::Board;
use othello_complexity_rs::lib::search::{retrospective_search, search, SearchResult};

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
    let total_input = boards.len();
    println!("info: read {} board(s) from '{}'.", total_input, input_path);

    // Ensure project-root `result` directory exists and write outputs there
    let result_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result");
    fs::create_dir_all(&result_dir)?;
    // Output files
    let mut ok = File::create(result_dir.join("reverse_OK.txt"))?;
    let mut ng = File::create(result_dir.join("reverse_NG.txt"))?;
    let mut unknown = File::create(result_dir.join("reverse_UNKNOWN.txt"))?;
    println!("info: writing outputs under '{}'", result_dir.display());

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

    // Node limit for reverse search (unique nodes). Configurable via MAX_NODES
    let node_limit: usize = env::var("MAX_NODES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000);
    // Determine parallelism n
    let n_threads: usize = env::var("N")
        .ok()
        .and_then(|s| s.parse().ok())
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(1)
        .max(1);
    println!("info: MAX_NODES = {}", node_limit);
    println!("info: N (threads) = {}", n_threads);

    // Share read-only leaf nodes across threads
    let leafnode_arc = Arc::new(leafnode);

    // 単一ワーカーならスレッド生成せず、メインスレッド（大きめのスタック）で順次処理
    if n_threads <= 1 {
        let mut retrospective_searched: HashSet<[u64; 2]> = HashSet::new();
        let mut retroflips: Vec<[u64; 10_000]> = vec![];
        let mut ok_c = 0usize;
        let mut ng_c = 0usize;
        let mut unknown_c = 0usize;
        for b in &boards {
            let line = b.to_string();
            if (b.player & b.opponent) != 0 {
                writeln!(ng, "{}", line)?;
                ng_c += 1;
                continue;
            }
            let occupied = b.player | b.opponent;
            if (occupied & CENTER_MASK) != CENTER_MASK {
                writeln!(ng, "{}", line)?;
                ng_c += 1;
                continue;
            }

            retrospective_searched.clear();
            match retrospective_search(
                b,
                false,
                discs,
                &*leafnode_arc,
                &mut retrospective_searched,
                &mut retroflips,
                node_limit,
            ) {
                SearchResult::Found => {
                    writeln!(ok, "{}", line)?;
                    ok_c += 1;
                }
                SearchResult::NotFound => {
                    writeln!(ng, "{}", line)?;
                    ng_c += 1;
                }
                SearchResult::Unknown => {
                    writeln!(unknown, "{}", line)?;
                    unknown_c += 1;
                }
            }
        }
        println!(
            "info: single-thread summary: processed = {}, OK = {}, NG = {}, UNKNOWN = {}",
            ok_c + ng_c + unknown_c,
            ok_c,
            ng_c,
            unknown_c
        );
        return Ok(());
    }

    // Split boards into n roughly-equal chunks and process in parallel
    let total = boards.len();
    let n_workers = n_threads.min(total.max(1));
    let chunk = (total + n_workers - 1) / n_workers; // ceil-div
    println!(
        "info: workers = {}, chunk = {}, total = {}",
        n_workers, chunk, total
    );

    // Collect results per category to write sequentially after joins
    let mut handles = Vec::with_capacity(n_workers);
    // スタックサイズはビルダーで拡張可能（既定 8MB）。深い再帰対策。
    let thread_stack_mb: usize = env::var("THREAD_STACK_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let stack_bytes = thread_stack_mb * 1024 * 1024;

    for i in 0..n_workers {
        let start = i * chunk;
        if start >= total {
            break;
        }
        let end = ((i + 1) * chunk).min(total);
        let slice: Vec<Board> = boards[start..end].to_vec();
        let leafnode_ref = Arc::clone(&leafnode_arc);
        let handle = thread::Builder::new()
            .name(format!("worker-{}", i))
            .stack_size(stack_bytes)
            .spawn(move || {
                let mut ok_lines: Vec<String> = Vec::new();
                let mut ng_lines: Vec<String> = Vec::new();
                let mut unknown_lines: Vec<String> = Vec::new();

                // Per-thread buffers for searches
                let mut retrospective_searched: HashSet<[u64; 2]> = HashSet::new();
                let mut retroflips: Vec<[u64; 10_000]> = vec![];

                for b in slice {
                    let line = b.to_string();

                    // quick sanity checks to avoid panics inside library helpers
                    if (b.player & b.opponent) != 0 {
                        ng_lines.push(line);
                        continue;
                    }
                    let occupied = b.player | b.opponent;
                    if (occupied & CENTER_MASK) != CENTER_MASK {
                        ng_lines.push(line);
                        continue;
                    }

                    // fresh visited set per board
                    retrospective_searched.clear();

                    match retrospective_search(
                        &b,
                        false,
                        discs,
                        &leafnode_ref,
                        &mut retrospective_searched,
                        &mut retroflips,
                        node_limit,
                    ) {
                        SearchResult::Found => ok_lines.push(line),
                        SearchResult::NotFound => ng_lines.push(line),
                        SearchResult::Unknown => unknown_lines.push(line),
                    }
                }

                let processed = ok_lines.len() + ng_lines.len() + unknown_lines.len();
                println!(
                    "info: worker done: boards = {}, OK = {}, NG = {}, UNKNOWN = {}",
                    processed,
                    ok_lines.len(),
                    ng_lines.len(),
                    unknown_lines.len()
                );

                (ok_lines, ng_lines, unknown_lines)
            })
            .expect("failed to spawn worker thread");
        handles.push(handle);
    }

    // Merge results and write once
    let mut ok_total = 0usize;
    let mut ng_total = 0usize;
    let mut unknown_total = 0usize;
    for h in handles {
        let (ok_lines, ng_lines, unknown_lines) = h.join().expect("thread panicked");
        for l in ok_lines {
            writeln!(ok, "{}", l)?;
            ok_total += 1;
        }
        for l in ng_lines {
            writeln!(ng, "{}", l)?;
            ng_total += 1;
        }
        for l in unknown_lines {
            writeln!(unknown, "{}", l)?;
            unknown_total += 1;
        }
    }
    println!(
        "info: total summary: processed = {}, OK = {}, NG = {}, UNKNOWN = {}",
        ok_total + ng_total + unknown_total,
        ok_total,
        ng_total,
        unknown_total
    );

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
