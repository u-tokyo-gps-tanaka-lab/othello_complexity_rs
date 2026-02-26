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

/// 単一スレッドでブロック単位の後退展開を実行
///
/// # パラメータ
/// - `num_disc`: 目標石数
/// - `tmp_dir`: 一時ディレクトリのパス
/// - `block_size`: 1ブロックあたりのレコード数
///
/// # 戻り値
/// - `Ok(true)`: 展開成功、前段の盤面が存在
/// - `Ok(false)`: 展開結果が空(探索終了条件)
/// - `Err`: I/Oエラー
///
/// # アルゴリズム詳細
/// - `r_{num_disc+1}.bin`を読み込み、総ブロック数を計算
/// - 各ブロックを順次expand_single_block()で処理
/// - 全ブロックファイルをmerge_files()で統合
/// - マージ結果が空なら後退探索の終端に到達
pub(in crate::search::external_bfs) fn expand_layer_blocked_seq(
    num_disc: i32,
    tmp_dir: &PathBuf,
    block_size: usize,
) -> Result<bool> {
    // 入力ファイル(1手先の盤面集合)を開いてサイズを検証
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let len = file.metadata()?.len() as usize;
    validate_file_size(len as u64, "expand_layer_blocked_seq")?;

    // 総レコード数(1レコード=16バイト)とブロック数を計算
    let all_count = len / 16;
    let block_count = (all_count + block_size - 1) / block_size;

    // 各ブロックを順次展開処理
    for i in 0..block_count {
        expand_single_block(num_disc, tmp_dir, block_size, i)?;
    }

    // 全ブロックファイルをマージして結果を確認
    let len = merge_files(num_disc, tmp_dir, block_count)?;
    if len == 0 {
        // 展開結果が空なら後退探索の終端に到達
        return Ok(false);
    }
    Ok(true)
}

/// 並列マルチスレッドでブロック単位の後退展開を実行
///
/// # パラメータ
/// - `num_disc`: 目標石数
/// - `tmp_dir`: 一時ディレクトリのパス
/// - `num_threads`: 使用するスレッド数
///
/// # 戻り値
/// - `Ok(true)`: 展開成功、前段の盤面が存在
/// - `Ok(false)`: 展開結果が空(探索終了条件)
/// - `Err`: I/Oエラーまたはワーカースレッドのパニック
///
/// # アルゴリズム詳細
/// - 動的ブロックサイズ決定: min(5M, max(1024, all_count/num_threads/10))
/// - 動的スケジューリング: AtomicUsizeでブロックインデックスを配布
/// - エラー検知時は即座に新規配布を停止(cancelフラグ)
/// - 全スレッドの完了を待ち、最初のエラーを返却
/// - マージフェーズは単一スレッドで実行
///
/// # 並列化戦略
/// - 各ワーカーはfetch_addで次のブロックを取得(work-stealing風)
/// - ブロックごとに独立処理可能なため競合なし
pub fn expand_layer_blocked(
    num_disc: i32,
    tmp_dir: &PathBuf,
    num_threads: usize,
) -> io::Result<bool> {
    // 入力ファイル(1手先の盤面集合)を開いてサイズを検証
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let len = file.metadata()?.len() as usize;
    validate_file_size(len as u64, "expand_layer_blocked")?;

    // 動的ブロックサイズとブロック数を計算
    // ブロックサイズ: min(5M, max(1024, 総レコード数/スレッド数/10))
    let all_count = len / 16;
    let block_size = std::cmp::min(5000000, std::cmp::max(1024, all_count / num_threads / 10));
    let block_count = (all_count + block_size - 1) / block_size;

    // --- 並列実行（動的スケジューリング） ---
    // 共有状態の初期化
    let next = Arc::new(AtomicUsize::new(0)); // 次に配布するブロックインデックス
    let cancel = Arc::new(AtomicBool::new(false)); // エラー検知時に新規配布を停止するフラグ
    let tdir = Arc::new(tmp_dir.clone());

    // ワーカースレッドを起動
    let mut handles = Vec::with_capacity(num_threads);
    for _ in 0..num_threads {
        let next = Arc::clone(&next);
        let cancel = Arc::clone(&cancel);
        let tdir = Arc::clone(&tdir);

        let handle = thread::spawn(move || -> io::Result<()> {
            loop {
                // キャンセルフラグが立っていれば終了
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                // fetch_addで次のブロックインデックスを動的に取得
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= block_count {
                    // 全ブロック処理済み
                    break;
                }
                // ブロックを展開処理
                if let Err(e) = expand_single_block(num_disc, &tdir, block_size, i) {
                    // エラー発生時はキャンセルフラグを立てて以降の配布を停止
                    cancel.store(true, Ordering::Relaxed);
                    return Err(e);
                }
            }
            Ok(())
        });
        handles.push(handle);
    }

    // 全スレッドの完了を待ち、最初のエラーを収集(panicも捕捉)
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

    // --- マージフェーズ ---
    // 全ブロックファイルを統合して最終結果を生成
    let out_len = merge_files(num_disc, &tdir, block_count)?;
    if out_len == 0 {
        // 展開結果が空なら後退探索の終端に到達
        return Ok(false);
    }
    Ok(true)
}

/// メモリ内で全盤面を展開する単純版(ブロック分割なし)
///
/// # パラメータ
/// - `num_disc`: 目標石数
/// - `tmp_dir`: 一時ディレクトリのパス
///
/// # 戻り値
/// - `Ok(true)`: 展開成功、前段の盤面が存在
/// - `Ok(false)`: 展開結果が空
/// - `Err`: I/Oエラー
///
/// # アルゴリズム詳細
/// - `r_{num_disc+1}.bin`を全件メモリ上のHashSetに展開
/// - process_board()で全レコードを処理
/// - 結果をソートして`r_{num_disc}.bin`に出力
pub(in crate::search::external_bfs) fn expand_layer_inplace(
    num_disc: i32,
    tmp_dir: &PathBuf,
) -> Result<bool> {
    // 入力ファイル(1手先の盤面集合)を開いてサイズを検証
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let len = file.metadata()?.len();
    validate_file_size(len, "expand_layer_inplace")?;

    // バッファと一時領域を初期化
    let mut r = BufReader::new(file);
    let mut buf = [0u8; 16];
    let nrecs = len / 16;
    println!("nrecs={}", nrecs);
    let mut prev_boards: HashSet<[u64; 2]> = HashSet::new(); // 1手前の盤面を格納
    let mut retroflips: [u64; 10_000] = [0u64; 10_000]; // 後退展開用の作業領域

    // 全レコードを読み込んで後退展開を実行
    for _ in 0..nrecs {
        r.read_exact(&mut buf)?;
        // バイト列を盤面データ(黒石・白石のビットボード)に変換
        let a = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
        let b = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
        // 1手前の盤面を計算してHashSetに追加
        process_board([a, b], &mut prev_boards, &mut retroflips);
    }

    // 展開結果が空なら後退探索の終端に到達
    if prev_boards.len() == 0 {
        return Ok(false);
    }

    // 結果をソートして出力ファイルに書き込み
    let mut bvec: Vec<[u64; 2]> = prev_boards.into_iter().collect();
    bvec.sort();
    let ofilename = format!("r_{}.bin", num_disc);
    let ofile = File::create(&tmp_dir.join(ofilename))?;
    let mut w = BufWriter::new(ofile);
    w.write_all(bytemuck::cast_slice(&bvec))?;
    w.flush()?;
    Ok(true)
}
