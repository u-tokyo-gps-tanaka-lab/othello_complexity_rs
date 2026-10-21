use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Error, ErrorKind, Result, Write};
use std::path::PathBuf;

use super::io::{read_header, read_position, validate_file_size, write_header, write_position};
use crate::othello::Position;

/// ソート済みの手番付き局面ファイル群を、
/// 重複を除去しながらマージしてoutputに書き出す
///
/// # パラメータ
/// - `inputs`: マージ対象のファイルパスのスライス
/// - `output`: 出力先ファイルパス
///
/// # 戻り値
/// - `Ok(count)`: 書き出したユニークレコード数
/// - `Err`: ファイルアクセスエラーまたは空入力エラー
///
/// # アルゴリズム詳細
/// - k-wayマージアルゴリズムを使用(k = inputs.len())
/// - min-heap(BinaryHeap<Reverse<...>>)で各ファイルの先頭要素を管理
/// - 最小要素をpopして出力し、該当ファイルから次要素を補充
/// - 連続する重複は最初の1つのみ出力(last変数で追跡)
pub fn merge_sorted_bins(inputs: &[PathBuf], output: &PathBuf) -> io::Result<usize> {
    if inputs.is_empty() {
        return Err(Error::new(ErrorKind::InvalidInput, "no input files"));
    }

    // 各入力ファイルのリーダを用意
    let mut readers: Vec<BufReader<File>> = Vec::with_capacity(inputs.len());
    for p in inputs {
        let file = File::open(p)?;
        validate_file_size(file.metadata()?.len(), "merge_sorted_bins")?;
        let mut reader = BufReader::new(file);
        read_header(&mut reader)?;
        readers.push(reader);
    }

    // min-heap: (position, file_idx)
    let mut heap: BinaryHeap<Reverse<(Position, usize)>> = BinaryHeap::new();

    // 各ファイルの先頭をヒープに積む
    for (i, r) in readers.iter_mut().enumerate() {
        if let Some(position) = read_position(r)? {
            heap.push(Reverse((position, i)));
        }
    }

    let outfile = File::create(output)?;
    let mut writer = BufWriter::new(outfile);
    write_header(&mut writer)?;

    let mut written: usize = 0;
    let mut last: Option<Position> = None;

    while let Some(Reverse((position, idx))) = heap.pop() {
        // 重複排除
        if last.map_or(true, |x| x != position) {
            write_position(&mut writer, position)?;
            last = Some(position);
            written += 1;
        }

        // 取り出したファイルから次レコードを補充
        if let Some(next) = read_position(&mut readers[idx])? {
            heap.push(Reverse((next, idx)));
        }
    }

    writer.flush()?;
    Ok(written)
}

/// 指定石数の全ブロックファイルをマージして単一のrファイルに統合
///
/// # パラメータ
/// - `num_disc`: 石数(ファイル名の命名に使用)
/// - `tmp_dir`: 一時ディレクトリのパス
/// - `block_count`: 処理されたブロック総数
///
/// # 戻り値
/// マージ後のユニークレコード数を返す。入力ファイルが存在しない場合は0。
///
/// # アルゴリズム詳細
/// - `b_{num_disc}_{0..block_count}.bin`を収集
/// - merge_sorted_bins()で統合マージ
/// - 出力を`r_{num_disc}.bin`として保存
/// - マージ後、元のブロックファイルを削除してディスク容量を節約
/// - 結果をstderrに出力(進捗モニタリング用)
pub(in crate::search::external_bfs) fn merge_files(
    num_disc: i32,
    tmp_dir: &PathBuf,
    block_count: usize,
) -> Result<usize> {
    let mut inputs: Vec<PathBuf> = vec![];
    for i in 0..block_count {
        let path = tmp_dir.join(format!("b_{}_{}.bin", num_disc, i));
        if path.exists() {
            inputs.push(path);
        }
    }
    let outfile = tmp_dir.join(format!("r_{}.bin", num_disc));
    let count = if inputs.is_empty() {
        write_header(&mut File::create(&outfile)?)?;
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
