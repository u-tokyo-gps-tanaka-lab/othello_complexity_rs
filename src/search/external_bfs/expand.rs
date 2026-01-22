use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, ErrorKind, Read, Result, Write};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::thread;

use bytemuck;

use super::io::validate_file_size;
use super::merge::merge_files;
use super::process::{expand_single_block, process_board};

pub(in crate::search::external_bfs) fn expand_layer_blocked_seq(
    num_disc: i32,
    tmp_dir: &PathBuf,
    block_size: usize,
) -> Result<bool> {
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let len = file.metadata()?.len() as usize;
    validate_file_size(len as u64, "expand_layer_blocked_seq")?;

    let all_count = len / 16;
    let block_count = (all_count + block_size - 1) / block_size;
    for i in 0..block_count {
        expand_single_block(num_disc, tmp_dir, block_size, i)?;
    }
    let len = merge_files(num_disc, tmp_dir, block_count)?;
    if len == 0 {
        return Ok(false);
    }
    Ok(true)
}

pub fn expand_layer_blocked(
    num_disc: i32,
    tmp_dir: &PathBuf,
    num_threads: usize,
) -> io::Result<bool> {
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let len = file.metadata()?.len() as usize;
    validate_file_size(len as u64, "expand_layer_blocked")?;

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
                if let Err(e) = expand_single_block(num_disc, &tdir, block_size, i) {
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

pub(in crate::search::external_bfs) fn expand_layer_inplace(
    num_disc: i32,
    tmp_dir: &PathBuf,
) -> Result<bool> {
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let len = file.metadata()?.len();
    validate_file_size(len, "expand_layer_inplace")?;

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
    let ofilename = format!("r_{}.bin", num_disc);
    let ofile = File::create(&tmp_dir.join(ofilename))?;
    let mut w = BufWriter::new(ofile);
    w.write_all(bytemuck::cast_slice(&bvec))?;
    w.flush()?;
    Ok(true)
}
