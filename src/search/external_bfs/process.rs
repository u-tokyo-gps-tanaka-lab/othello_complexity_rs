use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, ErrorKind, Read, Result, Seek, SeekFrom, Write};
use std::path::PathBuf;

use bytemuck;

use crate::othello::{get_moves, Board};
use crate::prunings::{occupancy::check_occupancy, seg3::check_seg3_more};
use crate::search::core::retrospective_flip;

use super::io::validate_file_size;

pub(in crate::search::external_bfs) fn process_board(
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
            if !check_occupancy(occupied) || !check_seg3_more(prev.player, prev.opponent) {
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
