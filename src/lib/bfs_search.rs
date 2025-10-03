use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Error, ErrorKind, Read, Result, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::thread;

use bytemuck;
use clap::Parser;

use crate::lib::othello::{flip, get_moves, Board, DXYS};
use crate::lib::search::{
    check_seg3, check_seg3_more, is_connected, retrospective_flip, SearchResult,
};

#[derive(Debug, Clone, Parser)]
#[command(name = "reverse_to_initial_bfs", version)]
pub struct Cfg {
    /// 入力ファイル
    pub input: PathBuf,

    /// 出力ディレクトリ
    #[arg(short, long, default_value = "result")]
    pub out_dir: PathBuf,

    /// スレッド数（0で自動）
    #[arg(short = 'j', long, default_value_t = 0)]
    pub jobs: usize,

    /// ログ詳細度
    #[arg(short, long, default_value_t = 0)]
    pub verbose: u8,

    /// ブロックサイズ
    #[arg(short = 'b', long, default_value_t = 1000000)]
    pub block_size: usize,

    /// forwardとreverseで合流する石数
    #[arg(short = 'd', long, default_value_t = 10)]
    pub discs: usize,

    /// tmp_dir
    #[arg(short = 't', long, default_value = "tmp")]
    pub tmp_dir: PathBuf,

    /// resume
    #[arg(short = 'r', long)]
    pub resume: bool,
}

fn process_board(
    board: [u64; 2],
    prev_boards: &mut HashSet<[u64; 2]>,
    retroflips: &mut [u64; 10_000],
) {
    let board: Board = Board::new(board[0], board[1]);
    let mut b = board.opponent & !0x0000_0018_1800_0000u64;
    if b == 0 {
        return;
    }
    while b != 0 {
        let index = b.trailing_zeros(); // 0..=63
        b &= b - 1;

        // “直前に相手が index に置いた” と想定したときの可能 flip 集合を列挙
        let num = retrospective_flip(index, board.player, board.opponent, retroflips);
        for i in 1..num {
            let flipped = retroflips[i];
            debug_assert!(flipped != 0);

            let prev = Board {
                // 直前に相手が index に置き、flipped が返ったと仮定した局面の 1 手前
                player: board.opponent ^ (flipped | (1u64 << index)),
                opponent: board.player ^ flipped,
            };
            let occupied = prev.player | prev.opponent;
            if !is_connected(occupied) {
                continue;
            }
            if !check_seg3(occupied) {
                continue;
            }
            if !check_seg3_more(prev.player, prev.opponent) {
                continue;
            }
            let uni = prev.unique();
            prev_boards.insert(uni);
            if get_moves(prev.opponent, prev.player) == 0 {
                let uni = Board::new(prev.opponent, prev.player).unique();
                prev_boards.insert(uni);
            }
        }
    }
}

fn process_bfs_block(
    num_disc: i32,
    tmp_dir: &PathBuf,
    block_size: usize,
    block_number: usize,
) -> Result<bool> {
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let mut file = File::open(&tmp_dir.join(rfilename))?;
    let meta = file.metadata()?;
    let len = meta.len() as usize;

    if len % 16 != 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("file size {} is not a multiple of 16 bytes", len),
        ));
    }
    let offset = block_size * block_number * 16;
    if offset >= len {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "block_size {} x block_number {} is greater than file size {}",
                block_size, block_number, len
            ),
        ));
    }
    file.seek(SeekFrom::Start(offset as u64))?;
    let mut r = BufReader::new(file);
    let mut buf = [0u8; 16];
    let nrecs = std::cmp::min(block_size, (len - offset) / 16);
    let mut prev_boards: HashSet<[u64; 2]> = HashSet::new();
    let mut retroflips: [u64; 10_000] = [0u64; 10_000];
    for _ in 0..nrecs {
        r.read_exact(&mut buf)?;
        let a = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
        let b = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
        process_board([a, b], &mut prev_boards, &mut retroflips);
    }
    if prev_boards.len() == 0 {
        return Ok(false);
    }
    let mut bvec: Vec<[u64; 2]> = prev_boards.into_iter().collect();
    bvec.sort();
    //eprintln!("num_disc={}, count={}", num_disc, bvec.len());
    let ofilename = format!("b_{}_{}.bin", num_disc, block_number);
    let ofile = File::create(&tmp_dir.join(ofilename))?;
    let mut w = BufWriter::new(ofile);
    w.write_all(bytemuck::cast_slice(&bvec))?;
    w.flush()?;
    Ok(true)
}

/// 1レコード (=16バイト) をネイティブエンディアンのまま読み取る
fn read_pair(reader: &mut BufReader<File>) -> io::Result<Option<(u64, u64)>> {
    let mut buf = [0u8; 16];
    // まず 1 バイト読んで EOF 判定を分ける（partial read 対策）
    match reader.read(&mut buf[..1])? {
        0 => return Ok(None), // EOF
        1 => {
            // すでに 1 バイト読んだので残り 15 バイト読む
            reader.read_exact(&mut buf[1..])?;
        }
        _ => unreachable!(),
    }
    let p = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
    let o = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
    Ok(Some((p, o)))
}

/// 1レコードを書き出し（ネイティブエンディアンのまま）
fn write_pair(writer: &mut BufWriter<File>, p: u64, o: u64) -> io::Result<()> {
    writer.write_all(&p.to_ne_bytes())?;
    writer.write_all(&o.to_ne_bytes())?;
    Ok(())
}

/// ソート済みの bin ファイル群（ネイティブエンディアンの [u64;2] 連続）を、
/// 重複を除去しながらマージして output に書き出す。
/// 返り値は「書き出したユニーク件数」。
pub fn merge_sorted_bins(inputs: &[PathBuf], output: &PathBuf) -> io::Result<usize> {
    if inputs.is_empty() {
        return Err(Error::new(ErrorKind::InvalidInput, "no input files"));
    }

    // 各入力ファイルのリーダを用意
    let mut readers: Vec<BufReader<File>> = Vec::with_capacity(inputs.len());
    for p in inputs {
        readers.push(BufReader::new(File::open(p)?));
    }

    // min-heap: (key=(p,o), file_idx)
    let mut heap: BinaryHeap<Reverse<((u64, u64), usize)>> = BinaryHeap::new();

    // 各ファイルの先頭をヒープに積む
    for (i, r) in readers.iter_mut().enumerate() {
        if let Some((p, o)) = read_pair(r)? {
            heap.push(Reverse(((p, o), i)));
        }
    }

    let outfile = File::create(output)?;
    let mut writer = BufWriter::new(outfile);

    let mut written: usize = 0;
    let mut last: Option<(u64, u64)> = None;

    while let Some(Reverse(((p, o), idx))) = heap.pop() {
        // 重複排除
        if last.map_or(true, |x| x != (p, o)) {
            write_pair(&mut writer, p, o)?;
            last = Some((p, o));
            written += 1;
        }

        // 取り出したファイルから次レコードを補充
        if let Some((np, no)) = read_pair(&mut readers[idx])? {
            heap.push(Reverse(((np, no), idx)));
        }
    }

    writer.flush()?;
    Ok(written)
}

fn merge_files(num_disc: i32, tmp_dir: &PathBuf, block_count: usize) -> Result<usize> {
    let mut inputs: Vec<PathBuf> = vec![];
    for i in 0..block_count {
        inputs.push(tmp_dir.join(format!("b_{}_{}.bin", num_disc, i)));
    }
    let outfile = tmp_dir.join(format!("r_{}.bin", num_disc));
    let count = merge_sorted_bins(&inputs, &outfile)?;
    for i in 0..inputs.len() {
        fs::remove_file(&inputs[i])?;
    }
    eprintln!("{} : {}", num_disc, count);
    Ok(count)
}

fn process_bfs_seq(num_disc: i32, tmp_dir: &PathBuf, block_size: usize) -> Result<bool> {
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let meta = file.metadata()?;
    let len = meta.len() as usize;

    if len % 16 != 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("file size {} is not a multiple of 16 bytes", len),
        ));
    }
    let all_count = len / 16;
    let block_count = (all_count + block_size - 1) / block_size;
    for i in 0..block_count {
        process_bfs_block(num_disc, tmp_dir, block_size, i)?;
    }
    let len = merge_files(num_disc, tmp_dir, block_count)?;
    if len == 0 {
        return Ok(false);
    }
    Ok(true)
}

pub fn process_bfs_par(num_disc: i32, tmp_dir: &PathBuf, num_threads: usize) -> io::Result<bool> {
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let len = file.metadata()?.len() as usize;

    if len % 16 != 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("file size {} is not a multiple of 16 bytes", len),
        ));
    }

    let all_count = len / 16;
    let block_size = std::cmp::min(5000000, std::cmp::max(1024, all_count / num_threads / 10));
    let block_count = (all_count + block_size - 1) / block_size;

    // --- 並列実行（動的スケジューリング） ---
    let next = Arc::new(AtomicUsize::new(0)); // 次に配る block index
    let cancel = Arc::new(AtomicBool::new(false)); // エラー検知で新規受付を止める
    let tdir = Arc::new(tmp_dir.clone());

    let mut handles = Vec::with_capacity(num_threads);
    for _ in 0..num_threads {
        let next = Arc::clone(&next);
        let cancel = Arc::clone(&cancel);
        let tdir = Arc::clone(&tdir);

        let handle = thread::spawn(move || -> io::Result<()> {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= block_count {
                    break;
                }
                if let Err(e) = process_bfs_block(num_disc, &tdir, block_size, i) {
                    // 以降の配布を止める
                    cancel.store(true, Ordering::Relaxed);
                    return Err(e);
                }
            }
            Ok(())
        });
        handles.push(handle);
    }

    // 最初のエラーを拾う（panic も拾う）
    let mut first_err: Option<io::Error> = None;
    for h in handles {
        match h.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            Err(_) => {
                if first_err.is_none() {
                    first_err = Some(io::Error::new(ErrorKind::Other, "worker thread panicked"));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }

    // --- マージ ---
    let out_len = merge_files(num_disc, &tdir, block_count)?;
    if out_len == 0 {
        return Ok(false);
    }
    Ok(true)
}
use std::num::NonZeroUsize;

fn available_threads() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1) // 取得失敗時のフォールバック
}

pub fn retrospective_search_bfs_par_resume(
    cfg: &Cfg,
    num_disc: i32,
    discs: i32,
    leafnode: &std::collections::HashSet<[u64; 2]>,
) -> Result<SearchResult> {
    let tmp_dir: &PathBuf = &cfg.tmp_dir;
    let mut jobs = cfg.jobs;
    if jobs == 0 {
        jobs = available_threads();
    }
    println!("parallelism = {}", jobs);
    for s in (discs..(num_disc as i32)).rev() {
        let v = process_bfs_par(s, tmp_dir, jobs)?;
        if !v {
            return Ok(SearchResult::NotFound);
        }
    }
    let rfilename = format!("r_{}.bin", discs);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let meta = file.metadata()?;
    let len = meta.len();

    if len % 16 != 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("file size {} is not a multiple of 16 bytes", len),
        ));
    }
    let mut r = BufReader::new(file);
    let mut buf = [0u8; 16];
    let nrecs = len / 16;
    let mut prev_boards: HashSet<[u64; 2]> = HashSet::new();
    let mut retroflips: [u64; 10_000] = [0u64; 10_000];
    for _ in 0..nrecs {
        r.read_exact(&mut buf)?;
        let a = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
        let b = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
        let uni = [a, b];
        if leafnode.contains(&uni) {
            return Ok(SearchResult::Found);
        }
    }
    Ok(SearchResult::NotFound)
}

//--------------------------------------
// 公開エントリ：ブロック版 retrospective
pub fn retrospective_search_bfs_par(
    cfg: &Cfg,
    board: &Board,
    discs: i32,
    leafnode: &std::collections::HashSet<[u64; 2]>,
) -> Result<SearchResult> {
    let uni = board.unique();
    let num_disc = board.popcount() as usize;
    let tmp_dir: &PathBuf = &cfg.tmp_dir;
    // let block_size = cfg.block_size;

    if (num_disc as i32) <= discs {
        return if leafnode.contains(&uni) {
            println!("info: found unique board in leafnodes:");
            println!("unique player = {}", uni[0]);
            println!("unique opponent = {}", uni[1]);
            println!("board player = {}", board.player);
            println!("board opponent = {}", board.opponent);
            Ok(SearchResult::Found)
        } else {
            Ok(SearchResult::NotFound)
        };
    }
    let mut boards: Vec<[u64; 2]> = vec![[board.player, board.opponent]];
    if get_moves(board.opponent, board.player) == 0 {
        boards.push([board.opponent, board.player]);
    }
    let rfilename = format!("r_{}.bin", num_disc);
    let rfile = File::create(&tmp_dir.join(rfilename))?;
    let mut w = BufWriter::new(rfile);
    w.write_all(bytemuck::cast_slice(&boards))?;
    w.flush()?;
    retrospective_search_bfs_par_resume(cfg, num_disc as i32, discs, leafnode)
}

//--------------------------------------
// 公開エントリ：ブロック版 retrospective
pub fn retrospective_search_bfs_seq(
    cfg: &Cfg,
    board: &Board,
    discs: i32,
    leafnode: &std::collections::HashSet<[u64; 2]>,
) -> Result<SearchResult> {
    let uni = board.unique();
    let num_disc = board.popcount() as usize;
    let tmp_dir: &PathBuf = &cfg.tmp_dir;
    let block_size = cfg.block_size;

    if (num_disc as i32) <= discs {
        return if leafnode.contains(&uni) {
            println!("info: found unique board in leafnodes:");
            println!("unique player = {}", uni[0]);
            println!("unique opponent = {}", uni[1]);
            println!("board player = {}", board.player);
            println!("board opponent = {}", board.opponent);
            Ok(SearchResult::Found)
        } else {
            Ok(SearchResult::NotFound)
        };
    }
    let mut boards: Vec<[u64; 2]> = vec![[board.player, board.opponent]];
    if get_moves(board.opponent, board.player) == 0 {
        boards.push([board.opponent, board.player]);
    }
    let rfilename = format!("r_{}.bin", num_disc);
    let rfile = File::create(&tmp_dir.join(rfilename))?;
    let mut w = BufWriter::new(rfile);
    w.write_all(bytemuck::cast_slice(&boards))?;
    w.flush()?;
    for s in (discs..(num_disc as i32)).rev() {
        let v = process_bfs_seq(s, tmp_dir, block_size)?;
        if !v {
            return Ok(SearchResult::NotFound);
        }
    }
    let rfilename = format!("r_{}.bin", discs);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let meta = file.metadata()?;
    let len = meta.len();

    if len % 16 != 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("file size {} is not a multiple of 16 bytes", len),
        ));
    }
    let mut r = BufReader::new(file);
    let mut buf = [0u8; 16];
    let nrecs = len / 16;
    let mut prev_boards: HashSet<[u64; 2]> = HashSet::new();
    let mut retroflips: [u64; 10_000] = [0u64; 10_000];
    for _ in 0..nrecs {
        r.read_exact(&mut buf)?;
        let a = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
        let b = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
        let uni = [a, b];
        if leafnode.contains(&uni) {
            return Ok(SearchResult::Found);
        }
    }
    Ok(SearchResult::NotFound)
}

fn process_bfs(num_disc: i32, tmp_dir: &PathBuf) -> Result<bool> {
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let meta = file.metadata()?;
    let len = meta.len();

    if len % 16 != 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("file size {} is not a multiple of 16 bytes", len),
        ));
    }
    let mut r = BufReader::new(file);
    let mut buf = [0u8; 16];
    let nrecs = len / 16;
    println!("nrecs={}", nrecs);
    let mut prev_boards: HashSet<[u64; 2]> = HashSet::new();
    let mut retroflips: [u64; 10_000] = [0u64; 10_000];
    for _ in 0..nrecs {
        r.read_exact(&mut buf)?;
        let a = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
        let b = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
        process_board([a, b], &mut prev_boards, &mut retroflips);
    }
    if prev_boards.len() == 0 {
        return Ok(false);
    }
    let mut bvec: Vec<[u64; 2]> = prev_boards.into_iter().collect();
    bvec.sort();
    // eprintln!("num_disc={}, count={}", num_disc, bvec.len());
    let ofilename = format!("r_{}.bin", num_disc);
    let ofile = File::create(&tmp_dir.join(ofilename))?;
    let mut w = BufWriter::new(ofile);
    w.write_all(bytemuck::cast_slice(&bvec))?;
    w.flush()?;
    Ok(true)
}

//--------------------------------------
// 公開エントリ：並列版 retrospective（シグネチャを分けました）
pub fn retrospective_search_bfs(
    cfg: &Cfg,
    board: &Board,
    discs: i32,
    leafnode: &std::collections::HashSet<[u64; 2]>,
) -> Result<SearchResult> {
    let uni = board.unique();
    let num_disc = board.popcount() as usize;
    let tmp_dir: &PathBuf = &cfg.tmp_dir;

    if (num_disc as i32) <= discs {
        return if leafnode.contains(&uni) {
            println!("info: found unique board in leafnodes:");
            println!("unique player = {}", uni[0]);
            println!("unique opponent = {}", uni[1]);
            println!("board player = {}", board.player);
            println!("board opponent = {}", board.opponent);
            Ok(SearchResult::Found)
        } else {
            Ok(SearchResult::NotFound)
        };
    }
    let mut boards: Vec<[u64; 2]> = vec![[board.player, board.opponent]];
    if get_moves(board.opponent, board.player) == 0 {
        boards.push([board.opponent, board.player]);
    }
    let rfilename = format!("r_{}.bin", num_disc);
    let rfile = File::create(&tmp_dir.join(rfilename))?;
    let mut w = BufWriter::new(rfile);
    w.write_all(bytemuck::cast_slice(&boards))?;
    w.flush()?;
    for s in (discs..(num_disc as i32)).rev() {
        let v = process_bfs(s, tmp_dir)?;
        if !v {
            return Ok(SearchResult::NotFound);
        }
    }
    let rfilename = format!("r_{}.bin", discs);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let meta = file.metadata()?;
    let len = meta.len();

    if len % 16 != 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("file size {} is not a multiple of 16 bytes", len),
        ));
    }
    let mut r = BufReader::new(file);
    let mut buf = [0u8; 16];
    let nrecs = len / 16;
    let mut prev_boards: HashSet<[u64; 2]> = HashSet::new();
    let mut retroflips: [u64; 10_000] = [0u64; 10_000];
    for _ in 0..nrecs {
        r.read_exact(&mut buf)?;
        let a = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
        let b = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
        let uni = [a, b];
        if leafnode.contains(&uni) {
            return Ok(SearchResult::Found);
        }
    }
    Ok(SearchResult::NotFound)
}
