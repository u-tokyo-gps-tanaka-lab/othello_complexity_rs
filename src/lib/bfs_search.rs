use clap::Parser;

use std::fs::{self, File};
use std::io::{self, BufWriter, Write, Result, BufReader, Read, ErrorKind};
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use bytemuck;


use crate::lib::othello::{flip, get_moves, Board, DXYS};
use crate::lib::search::{SearchResult, retrospective_flip, check_seg3, is_connected};


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

    /// ログ詳細度
    #[arg(short = 'd', long, default_value_t = 10)]
    pub discs: usize,

    /// tmp_dir
    #[arg(short = 't', long, default_value = "tmp")]
    pub tmp_dir: PathBuf,
}

fn process_board(board: [u64;2], prev_boards: &mut HashSet<[u64;2]>, retroflips: &mut [u64; 10_000]) {
    let board: Board = Board::new(board[0], board[1]);
    let mut b = board.opponent & !0x0000_0018_1800_0000u64;
    if b == 0 {
        return;
    }
    while b != 0 {
        let index = b.trailing_zeros(); // 0..=63
        b &= b - 1;

        // “直前に相手が index に置いた” と想定したときの可能 flip 集合を列挙
        let num = retrospective_flip(
            index,
            board.player,
            board.opponent,
            retroflips,
        );
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
            let uni = prev.unique();
            prev_boards.insert(uni);
            if get_moves(prev.opponent, prev.player) == 0 {
                let uni = Board::new(prev.opponent, prev.player).unique();
                prev_boards.insert(uni);
            }
        }            
    }
}

fn process_bfs(num_disc: i32, tmp_dir: &PathBuf) -> Result<bool> {
    let rfilename = format!("r_{}.bin",num_disc + 1);
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
    let mut prev_boards: HashSet<[u64;2]> = HashSet::new();
    let mut retroflips: [u64; 10_000] = [0u64;10_000];
    for _ in 0..nrecs {
        r.read_exact(&mut buf)?;
        let a = u64::from_ne_bytes(buf[0..8].try_into().unwrap());
        let b = u64::from_ne_bytes(buf[8..16].try_into().unwrap());
        process_board([a, b], &mut prev_boards, &mut retroflips);
    }
    if prev_boards.len() == 0 {
        return Ok(false)
    }
    let mut bvec: Vec<[u64;2]> = prev_boards.into_iter().collect();
    bvec.sort();
    eprintln!("num_disc={}, count={}", num_disc, bvec.len());
    let ofilename = format!("r_{}.bin",num_disc);
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
    let mut boards: Vec<[u64;2]> = vec![[board.player, board.opponent]];
    if get_moves(board.opponent, board.player) == 0 {
        boards.push([board.opponent, board.player]);
    }
    let rfilename = format!("r_{}.bin",num_disc);
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
    let mut prev_boards: HashSet<[u64;2]> = HashSet::new();
    let mut retroflips: [u64; 10_000] = [0u64;10_000];
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