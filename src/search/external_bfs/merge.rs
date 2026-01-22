use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Error, ErrorKind, Result, Write};
use std::path::PathBuf;

use super::io::{read_pair, write_pair};

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

pub(in crate::search::external_bfs) fn merge_files(num_disc: i32, tmp_dir: &PathBuf, block_count: usize) -> Result<usize> {
    let mut inputs: Vec<PathBuf> = vec![];
    for i in 0..block_count {
        let path = tmp_dir.join(format!("b_{}_{}.bin", num_disc, i));
        if path.exists() {
            inputs.push(path);
        }
    }
    let outfile = tmp_dir.join(format!("r_{}.bin", num_disc));
    let count = if inputs.is_empty() {
        File::create(&outfile)?;
        0
    } else {
        let c = merge_sorted_bins(&inputs, &outfile)?;
        for p in &inputs {
            fs::remove_file(p)?;
        }
        c
    };
    eprintln!("{} : {}", num_disc, count);
    Ok(count)
}
