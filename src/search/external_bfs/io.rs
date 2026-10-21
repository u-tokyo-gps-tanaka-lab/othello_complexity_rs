use std::fs::File;
use std::io::{self, BufReader, BufWriter, Error, ErrorKind, Read, Result, Write};
use std::path::PathBuf;

use crate::othello::{get_moves, validate_board, Board, Color, Position};
use crate::search::core::SearchResult;

// Each file starts with the format identifier; each position is two u64 values
// in little-endian order followed by its absolute turn (0 = Black, 1 = White).
pub(super) const HEADER_SIZE: u64 = 8;
pub(super) const RECORD_SIZE: u64 = 17;
const MAGIC: &[u8; 8] = b"OTHRBP01";

pub(super) fn write_header(writer: &mut impl Write) -> Result<()> {
    writer.write_all(MAGIC)
}

pub(super) fn read_header(reader: &mut impl Read) -> Result<()> {
    let mut header = [0; 8];
    reader.read_exact(&mut header)?;
    if &header != MAGIC {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid absolute-position file header",
        ));
    }
    Ok(())
}

pub(in crate::search::external_bfs) fn validate_file_size(len: u64, context: &str) -> Result<()> {
    if len < HEADER_SIZE || (len - HEADER_SIZE) % RECORD_SIZE != 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("{context}: incomplete absolute-position records in {len} bytes"),
        ));
    }
    Ok(())
}

/// Save the current stone layer, including a legal reverse pass at its root.
pub(in crate::search::external_bfs) fn write_given_board(
    tmp_dir: &PathBuf,
    position: &Position,
    num_disc: usize,
) -> Result<()> {
    let board = position.board;
    let mut positions = vec![*position];
    if get_moves(board.opponent, board.player) == 0 && get_moves(board.player, board.opponent) != 0
    {
        positions.push(Position {
            board: board.swapped(),
            side_to_move: position.side_to_move.opposite(),
        });
    }
    let rfile = File::create(tmp_dir.join(format!("r_{num_disc}.bin")))?;
    let mut writer = BufWriter::new(rfile);
    write_header(&mut writer)?;
    for position in positions {
        write_position(&mut writer, position)?;
    }
    writer.flush()
}

/// Save the exact query position for the existing external-BFS resume path.
pub(in crate::search::external_bfs) fn write_target_board(
    tmp_dir: &PathBuf,
    position: &Position,
) -> Result<()> {
    let mut writer = BufWriter::new(File::create(tmp_dir.join("target_board.bin"))?);
    write_header(&mut writer)?;
    write_position(&mut writer, *position)?;
    writer.flush()
}

pub fn read_target_board(tmp_dir: &PathBuf) -> Result<Position> {
    let file = File::open(tmp_dir.join("target_board.bin"))?;
    if file.metadata()?.len() != HEADER_SIZE + RECORD_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "expected one saved target position",
        ));
    }
    let mut reader = BufReader::new(file);
    read_header(&mut reader)?;
    read_position(&mut reader)?
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "missing target position"))
}

pub(in crate::search::external_bfs) fn check_leafnode_match(
    tmp_dir: &PathBuf,
    discs: i32,
    leafnode: &Vec<Position>,
) -> Result<SearchResult> {
    let file = File::open(tmp_dir.join(format!("r_{discs}.bin")))?;
    validate_file_size(file.metadata()?.len(), "check_leafnode_match")?;
    let mut reader = BufReader::new(file);
    read_header(&mut reader)?;
    while let Some(position) = read_position(&mut reader)? {
        if leafnode.binary_search(&position).is_ok() {
            return Ok(SearchResult::Found);
        }
    }
    Ok(SearchResult::NotFound)
}

pub(crate) fn read_position(reader: &mut BufReader<File>) -> io::Result<Option<Position>> {
    let mut buf = [0; 17];
    match reader.read(&mut buf[..1])? {
        0 => return Ok(None),
        1 => reader.read_exact(&mut buf[1..])?,
        _ => unreachable!(),
    }
    let board = Board::new(
        u64::from_le_bytes(buf[..8].try_into().unwrap()),
        u64::from_le_bytes(buf[8..16].try_into().unwrap()),
    );
    validate_board(&board).map_err(|error| {
        Error::new(
            ErrorKind::InvalidData,
            format!("invalid saved board: {error:?}"),
        )
    })?;
    let side_to_move = match buf[16] {
        0 => Color::Black,
        1 => Color::White,
        _ => return Err(Error::new(ErrorKind::InvalidData, "invalid absolute turn")),
    };
    Ok(Some(Position {
        board,
        side_to_move,
    }))
}

pub(crate) fn write_position(writer: &mut BufWriter<File>, position: Position) -> io::Result<()> {
    writer.write_all(&position.board.player.to_le_bytes())?;
    writer.write_all(&position.board.opponent.to_le_bytes())?;
    writer.write_all(&[match position.side_to_move {
        Color::Black => 0,
        Color::White => 1,
    }])
}
