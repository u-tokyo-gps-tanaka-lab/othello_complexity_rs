use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, ErrorKind, Read, Result, Seek, SeekFrom, Write};
use std::path::PathBuf;

use bytemuck;

use crate::othello::{get_moves, Board, CENTER_MASK};
use crate::prunings::impossible_edges::check_edge_patterns;
use crate::prunings::kissat::is_sat_ok;
use crate::prunings::linear_programming::check_lp;
use crate::prunings::{occupancy::check_occupancy, seg3::check_seg3_more};
use crate::search::core::retrospective_flip;

use super::io::validate_file_size;

/// 単一盤面に対して後退展開を実行し、1手前の盤面集合を生成
///
/// # パラメータ
/// - `board`: 処理対象の盤面 [player, opponent]
/// - `prev_boards`: 生成された1手前の盤面を格納するHashSet
/// - `retroflips`: retrospective_flip関数の作業用バッファ(10,000要素)
///
/// # アルゴリズム詳細
/// - 中央4マス以外の全ての石について「相手が直前にここに置いた」と仮定
/// - 各位置に対してretrospective_flip()で可能な反転パターンを列挙
/// - 反転を適用して1手前の盤面を復元
/// - occupancy検証とseg3枝刈りで到達不可能な盤面を除外
/// - パス判定を行い、必要に応じて盤面を反転して追加
///
/// # 注意
/// - 中央4マス(0x0000_0018_1800_0000)は初期配置なので対象外
pub(in crate::search::external_bfs) fn process_board(
    board: [u64; 2],
    prev_boards: &mut HashSet<[u64; 2]>,
    retroflips: &mut [u64; 10_000],
) {
    let board: Board = Board::new(board[0], board[1]);
    let mut b = board.opponent & !CENTER_MASK;
    if b == 0 {
        return;
    }
    while b != 0 {
        let index = b.trailing_zeros() as usize; // 0..=63
        b &= b - 1;

        // "直前に相手が index に置いた" と想定したときの可能 flip 集合を列挙
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
            if !check_occupancy(occupied)
                || !check_seg3_more(prev.player, prev.opponent)
                || !check_edge_patterns(prev.player, prev.opponent)
                // || !is_sat_ok(prev.player, prev.opponent, false).unwrap_or(true)
                || !check_lp(prev.player, prev.opponent, false)
            {
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

/// 指定されたブロックを読み込んで後退展開し、ソート済みファイルとして出力
///
/// # パラメータ
/// - `num_disc`: 目標石数(出力ファイルの命名に使用)
/// - `tmp_dir`: 一時ディレクトリのパス
/// - `block_size`: 1ブロックあたりのレコード数
/// - `block_number`: 処理するブロックのインデックス(0始まり)
///
/// # 戻り値
/// - `Ok(true)`: ブロック処理が成功し、前段の盤面が見つかった
/// - `Ok(false)`: 有効な前段盤面が生成されなかった
/// - `Err`: ファイルアクセスエラーやブロック範囲外エラー
///
/// # アルゴリズム詳細
/// - `r_{num_disc+1}.bin`から指定ブロックをシーク読み込み
/// - 各レコードにprocess_board()を適用してHashSetに集約
/// - HashSetをVecに変換してソート(マージ効率化のため)
/// - `b_{num_disc}_{block_number}.bin`として出力
///
/// # メモリ効率
/// - ブロック単位の処理により、全体をメモリに展開せず外部メモリ探索を実現
pub(in crate::search::external_bfs) fn expand_single_block(
    num_disc: i32,
    tmp_dir: &PathBuf,
    block_size: usize,
    block_number: usize,
) -> Result<bool> {
    let rfilename = format!("r_{}.bin", num_disc + 1);
    let mut file = File::open(&tmp_dir.join(rfilename))?;
    let len = file.metadata()?.len() as usize;
    validate_file_size(len as u64, "expand_single_block")?;

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
    let ofilename = format!("b_{}_{}.bin", num_disc, block_number);
    let ofile = File::create(&tmp_dir.join(ofilename))?;
    let mut w = BufWriter::new(ofile);
    w.write_all(bytemuck::cast_slice(&bvec))?;
    w.flush()?;
    Ok(true)
}
