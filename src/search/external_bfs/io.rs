use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Error, ErrorKind, Read, Result, Write};
use std::path::PathBuf;

use bytemuck;

use crate::othello::{get_moves, Board};
use crate::search::core::SearchResult;

/// ファイルサイズが16バイトの倍数であることを検証
pub(in crate::search::external_bfs) fn validate_file_size(len: u64, context: &str) -> Result<()> {
    if len % 16 != 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "{}: file size {} is not a multiple of 16 bytes",
                context, len
            ),
        ));
    }
    Ok(())
}

/// 探索すべき局面をファイルに書き出す
pub(in crate::search::external_bfs) fn write_given_board(
    tmp_dir: &PathBuf,
    board: &Board,
    num_disc: usize,
) -> Result<()> {
    let mut boards: Vec<[u64; 2]> = vec![[board.player, board.opponent]];
    if get_moves(board.opponent, board.player) == 0 {
        boards.push([board.opponent, board.player]);
    }
    let rfilename = format!("r_{}.bin", num_disc);
    let rfile = File::create(&tmp_dir.join(rfilename))?;
    let mut w = BufWriter::new(rfile);
    w.write_all(bytemuck::cast_slice(&boards))?;
    w.flush()?;
    Ok(())
}

/// 最終結果ファイルをleafnodeと照合
pub(in crate::search::external_bfs) fn check_leafnode_match(
    tmp_dir: &PathBuf,
    discs: i32,
    leafnode: &HashSet<[u64; 2]>,
) -> Result<SearchResult> {
    let rfilename = format!("r_{}.bin", discs);
    let file = File::open(&tmp_dir.join(rfilename))?;
    let len = file.metadata()?.len();
    validate_file_size(len, "check_leafnode_match")?;

    let mut r = BufReader::new(file);
    let mut buf = [0u8; 16];
    let nrecs = len / 16;

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

/// 1レコード (=16バイト) をネイティブエンディアンのまま読み取る
pub(crate) fn read_pair(reader: &mut BufReader<File>) -> io::Result<Option<(u64, u64)>> {
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
pub(crate) fn write_pair(writer: &mut BufWriter<File>, p: u64, o: u64) -> io::Result<()> {
    writer.write_all(&p.to_ne_bytes())?;
    writer.write_all(&o.to_ne_bytes())?;
    Ok(())
}
