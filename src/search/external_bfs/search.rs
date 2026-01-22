use std::collections::HashSet;
use std::io::Result;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use crate::othello::Board;
use crate::search::core::SearchResult;

use super::config::Cfg;
use super::expand::{expand_layer_blocked, expand_layer_blocked_seq, expand_layer_inplace};
use super::io::{check_leafnode_match, write_given_board};

fn available_threads() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1) // 取得失敗時のフォールバック
}

pub fn retrospective_search_bfs_par_resume(
    cfg: &Cfg,
    num_disc: i32,
    discs: i32,
    leafnode: &HashSet<[u64; 2]>,
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

pub fn retrospective_search_bfs_par(
    cfg: &Cfg,
    board: &Board,
    discs: i32,
    leafnode: &HashSet<[u64; 2]>,
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
    write_given_board(tmp_dir, board, num_disc)?;
    retrospective_search_bfs_par_resume(cfg, num_disc as i32, discs, leafnode)
}

pub fn retrospective_search_bfs_seq(
    cfg: &Cfg,
    board: &Board,
    discs: i32,
    leafnode: &HashSet<[u64; 2]>,
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
    write_given_board(tmp_dir, board, num_disc)?;
    for s in (discs..(num_disc as i32)).rev() {
        let v = expand_layer_blocked_seq(s, tmp_dir, block_size)?;
        if !v {
            return Ok(SearchResult::NotFound);
        }
    }
    check_leafnode_match(tmp_dir, discs, leafnode)
}

pub fn retrospective_search_bfs(
    cfg: &Cfg,
    board: &Board,
    discs: i32,
    leafnode: &HashSet<[u64; 2]>,
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
    write_given_board(tmp_dir, board, num_disc)?;
    for s in (discs..(num_disc as i32)).rev() {
        let v = expand_layer_inplace(s, tmp_dir)?;
        if !v {
            return Ok(SearchResult::NotFound);
        }
    }
    check_leafnode_match(tmp_dir, discs, leafnode)
}
