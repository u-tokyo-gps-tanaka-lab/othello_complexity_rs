use std::io::Result;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use crate::othello::Board;
use crate::search::core::SearchResult;

use super::config::Cfg;
use super::expand::{expand_layer_blocked, expand_layer_blocked_seq, expand_layer_inplace};
use super::io::{check_leafnode_match, write_given_board, write_target_board};

/// システムで利用可能な並列度(論理コア数)を取得
///
/// # 戻り値
/// 利用可能なスレッド数。取得失敗時はフォールバックとして1を返す。
fn available_threads() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1) // 取得失敗時のフォールバック
}

/// 中断された並列BFS後退探索を再開
///
/// # パラメータ
/// - `cfg`: 探索設定(スレッド数、一時ディレクトリなど)
/// - `num_disc`: 現在の石数(再開開始点)
/// - `discs`: 目標石数(前進探索との合流点)
/// - `leafnode`: 前進探索で生成された到達可能盤面のソート済みVec
///
/// # 戻り値
/// - `SearchResult::Found`: リーフノードとの合流に成功
/// - `SearchResult::NotFound`: 後退探索が終端に到達(合流失敗)
/// - `Err`: I/Oエラー
///
/// # アルゴリズム詳細
/// - num_discからdiscsまで逆順に層を展開
/// - 各層でexpand_layer_blocked()を並列実行
/// - 最終層でcheck_leafnode_match()による合流判定
///
/// # レジューム機能
/// - 既存の`r_{num_disc}.bin`から再開
/// - 中間ファイルが残っている前提
pub fn parallel_retrospective_bfs_resume(
    cfg: &Cfg,
    num_disc: i32,
    discs: i32,
    leafnode: &Vec<[u64; 2]>,
) -> Result<SearchResult> {
    let tmp_dir: &PathBuf = &cfg.tmp_dir;
    let mut jobs = cfg.jobs;
    if jobs == 0 {
        jobs = available_threads();
    }
    println!("parallelism = {}", jobs);
    for s in (discs..(num_disc as i32)).rev() {
        let v = expand_layer_blocked(s, tmp_dir, jobs)?;
        if !v {
            return Ok(SearchResult::NotFound);
        }
    }
    check_leafnode_match(tmp_dir, discs, leafnode)
}

/// 指定盤面から並列BFS後退探索を実行
///
/// # パラメータ
/// - `cfg`: 探索設定
/// - `board`: 探索開始盤面
/// - `discs`: 目標石数(前進探索との合流点)
/// - `leafnode`: 前進探索で生成された到達可能盤面のソート済みVec
///
/// # 戻り値
/// - `SearchResult::Found`: 合流成功、盤面は到達可能
/// - `SearchResult::NotFound`: 合流失敗、盤面は到達不可能
/// - `Err`: I/Oエラー
///
/// # アルゴリズム詳細
/// - 盤面の石数がdiscs以下の場合は直接リーフノード照合
/// - それ以外はwrite_given_board()で初期化後、_resumeを呼び出し
/// - 並列度はcfg.jobsで指定(0の場合は自動設定)
///
/// # 探索戦略
/// - 双方向探索: 前進(初期盤面から)と後退(目標盤面から)を組み合わせ
/// - 中間層(discs石)で合流判定を行うことで探索空間を削減
pub fn parallel_retrospective_bfs(
    cfg: &Cfg,
    board: &Board,
    discs: i32,
    leafnode: &Vec<[u64; 2]>,
) -> Result<SearchResult> {
    let uni = board.unique();
    let num_disc = board.popcount() as usize;
    let tmp_dir: &PathBuf = &cfg.tmp_dir;

    if (num_disc as i32) <= discs {
        return if leafnode.binary_search(&uni).is_ok() {
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
    write_target_board(tmp_dir, board)?;
    write_given_board(tmp_dir, board, num_disc)?;
    parallel_retrospective_bfs_resume(cfg, num_disc as i32, discs, leafnode)
}

/// 指定盤面から単一スレッドBFS後退探索を実行
///
/// # パラメータ
/// - `cfg`: 探索設定(block_sizeを使用)
/// - `board`: 探索開始盤面
/// - `discs`: 目標石数
/// - `leafnode`: 前進探索で生成された到達可能盤面のソート済みVec
///
/// # 戻り値
/// 並列版と同様
///
/// # アルゴリズム詳細
/// - expand_layer_blocked_seq()を使用(単一スレッド版)
/// - ブロックサイズはcfg.block_sizeで明示的に指定
///
/// # 使用場面
/// - デバッグ、検証用途
pub fn sequential_retrospective_bfs(
    cfg: &Cfg,
    board: &Board,
    discs: i32,
    leafnode: &Vec<[u64; 2]>,
) -> Result<SearchResult> {
    let uni = board.unique();
    let num_disc = board.popcount() as usize;
    let tmp_dir: &PathBuf = &cfg.tmp_dir;
    let block_size = cfg.block_size;

    if (num_disc as i32) <= discs {
        return if leafnode.binary_search(&uni).is_ok() {
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
    write_given_board(tmp_dir, board, num_disc)?;
    for s in (discs..(num_disc as i32)).rev() {
        let v = expand_layer_blocked_seq(s, tmp_dir, block_size)?;
        if !v {
            return Ok(SearchResult::NotFound);
        }
    }
    check_leafnode_match(tmp_dir, discs, leafnode)
}

/// インプレース方式(ブロック分割なし)でBFS後退探索を実行
///
/// # パラメータ
/// - `cfg`: 探索設定
/// - `board`: 探索開始盤面
/// - `discs`: 目標石数
/// - `leafnode`: 前進探索で生成された到達可能盤面のソート済みVec
///
/// # 戻り値
/// 並列版と同様
///
/// # アルゴリズム詳細
/// - expand_layer_inplace()を使用(全メモリ展開)
/// - ブロック分割やマージ処理なし
///
/// # 制約
/// - 小規模探索専用
/// - 大規模状態空間ではメモリ不足の可能性
pub fn unblocked_retrospective_bfs(
    cfg: &Cfg,
    board: &Board,
    discs: i32,
    leafnode: &Vec<[u64; 2]>,
) -> Result<SearchResult> {
    let uni = board.unique();
    let num_disc = board.popcount() as usize;
    let tmp_dir: &PathBuf = &cfg.tmp_dir;

    if (num_disc as i32) <= discs {
        return if leafnode.binary_search(&uni).is_ok() {
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
    write_given_board(tmp_dir, board, num_disc)?;
    for s in (discs..(num_disc as i32)).rev() {
        let v = expand_layer_inplace(s, tmp_dir)?;
        if !v {
            return Ok(SearchResult::NotFound);
        }
    }
    check_leafnode_match(tmp_dir, discs, leafnode)
}
