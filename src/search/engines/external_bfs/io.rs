//! Portable stone-layer files: magic, removed-stone count u32 LE, record count
//! u64 LE, context length u32 LE, UTF-8 JSON context, then fixed-size records.
//! Each 17 B record contains two u64 LE bitboards and a turn byte. Records are
//! strictly ordered by decoded `Position`, not by their byte representation.

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::ExternalBfs;
use crate::io::format_position;
use crate::othello::{validate_board, Board, Color, Position};
use crate::record::GoalMatch;

pub(super) const MAGIC: &[u8; 8] = b"OTHRBF03";
const RECORD_SIZE: u64 = 17;

pub(super) fn identity(
    cfg: &ExternalBfs,
    goal: Position,
    goal_match: GoalMatch,
    parallel: bool,
) -> Value {
    json!({"goal": format_position(goal),
        "goal_match": goal_match.as_str(),
        "discs": cfg.discs, "block_size": cfg.block_size,
        "jobs": cfg.jobs, "parallel": parallel})
}

pub(super) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(super) struct PositionWriter {
    writer: BufWriter<File>,
    previous: Option<Position>,
    count: u64,
}

impl PositionWriter {
    pub fn create(path: &Path, context: &Value, depth: usize) -> io::Result<Self> {
        if depth > 60 {
            return Err(invalid("stone depth exceeds the Othello bound"));
        }
        let data = serde_json::to_vec(context).map_err(invalid_json)?;
        let context_length =
            u32::try_from(data.len()).map_err(|_| invalid("checkpoint context is too large"))?;
        let mut writer = BufWriter::new(File::create(path)?);
        writer.write_all(MAGIC)?;
        writer.write_all(&(depth as u32).to_le_bytes())?;
        writer.write_all(&0_u64.to_le_bytes())?;
        writer.write_all(&context_length.to_le_bytes())?;
        writer.write_all(&data)?;
        Ok(Self {
            writer,
            previous: None,
            count: 0,
        })
    }

    pub fn push(&mut self, position: &Position) -> io::Result<()> {
        if self.previous.is_some_and(|previous| previous >= *position) {
            return Err(invalid(
                "external BFS positions are not in strictly increasing order",
            ));
        }
        validate_board(&position.board)
            .map_err(|error| invalid(format!("invalid saved position: {error:?}")))?;
        self.writer
            .write_all(&position.board.player.to_le_bytes())?;
        self.writer
            .write_all(&position.board.opponent.to_le_bytes())?;
        self.writer.write_all(&[match position.side_to_move {
            Color::Black => 0,
            Color::White => 1,
        }])?;
        self.previous = Some(*position);
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| invalid("external BFS record count overflow"))?;
        Ok(())
    }

    pub fn finish(self) -> io::Result<u64> {
        self.finish_inner(false)
    }

    pub fn finish_synced(self) -> io::Result<u64> {
        self.finish_inner(true)
    }

    fn finish_inner(mut self, sync: bool) -> io::Result<u64> {
        self.writer.flush()?;
        let mut file = self
            .writer
            .into_inner()
            .map_err(|error| error.into_error())?;
        file.seek(SeekFrom::Start(12))?;
        file.write_all(&self.count.to_le_bytes())?;
        if sync {
            file.sync_all()?;
        }
        Ok(self.count)
    }
}

pub(super) struct PositionReader {
    reader: BufReader<File>,
    depth: usize,
    remaining: u64,
    count: u64,
    goal: Position,
    previous: Option<Position>,
}

impl PositionReader {
    pub fn open(path: &Path, context: &Value, depth: usize, goal: Position) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let header = read_header(&mut file, context, depth)?;
        Ok(Self {
            reader: BufReader::new(file),
            depth,
            remaining: header.count,
            count: header.count,
            goal,
            previous: None,
        })
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn next(&mut self) -> io::Result<Option<Position>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let position = read_position(&mut self.reader, self.depth, self.goal)?;
        if self.previous.is_some_and(|previous| previous >= position) {
            return Err(invalid(
                "external BFS positions are not in strictly increasing order",
            ));
        }
        self.previous = Some(position);
        self.remaining -= 1;
        Ok(Some(position))
    }
}

pub(super) fn contains_position(
    path: &Path,
    context: &Value,
    depth: usize,
    goal: Position,
    target: Position,
) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let header = read_header(&mut file, context, depth)?;
    let mut lower = 0_u64;
    let mut upper = header.count;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let offset = middle
            .checked_mul(RECORD_SIZE)
            .and_then(|offset| header.data_offset.checked_add(offset))
            .ok_or_else(|| invalid("external BFS record offset overflow"))?;
        file.seek(SeekFrom::Start(offset))?;
        let position = read_position(&mut file, depth, goal)?;
        match position.cmp(&target) {
            Ordering::Less => lower = middle + 1,
            Ordering::Equal => return Ok(true),
            Ordering::Greater => upper = middle,
        }
    }
    Ok(false)
}

struct Header {
    count: u64,
    data_offset: u64,
}

fn read_header(file: &mut File, context: &Value, depth: usize) -> io::Result<Header> {
    let bytes = file.metadata()?.len();
    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid("unrecognized external BFS file header"));
    }
    let mut u32_bytes = [0; 4];
    let mut u64_bytes = [0; 8];
    file.read_exact(&mut u32_bytes)?;
    let stored_depth = u32::from_le_bytes(u32_bytes) as usize;
    if stored_depth != depth || depth > 60 {
        return Err(invalid("checkpoint stone depth mismatch"));
    }
    file.read_exact(&mut u64_bytes)?;
    let count = u64::from_le_bytes(u64_bytes);
    file.read_exact(&mut u32_bytes)?;
    let context_length = u32::from_le_bytes(u32_bytes) as usize;
    if context_length > 1_000_000 {
        return Err(invalid("checkpoint context is too large"));
    }
    let mut data = vec![0; context_length];
    file.read_exact(&mut data)?;
    let stored: Value = serde_json::from_slice(&data).map_err(invalid_json)?;
    if &stored != context {
        return Err(invalid("checkpoint query or search settings mismatch"));
    }
    let data_offset = 24_u64
        .checked_add(context_length as u64)
        .ok_or_else(|| invalid("external BFS header length overflow"))?;
    let expected = count
        .checked_mul(RECORD_SIZE)
        .and_then(|records| data_offset.checked_add(records))
        .ok_or_else(|| invalid("external BFS file length overflow"))?;
    if bytes != expected {
        return Err(invalid("incomplete or extra external BFS records"));
    }
    Ok(Header { count, data_offset })
}

fn read_position(reader: &mut impl Read, depth: usize, goal: Position) -> io::Result<Position> {
    let mut bits = [0; 8];
    reader.read_exact(&mut bits)?;
    let player = u64::from_le_bytes(bits);
    reader.read_exact(&mut bits)?;
    let opponent = u64::from_le_bytes(bits);
    let mut turn = [0];
    reader.read_exact(&mut turn)?;
    let side_to_move = match turn[0] {
        0 => Color::Black,
        1 => Color::White,
        _ => return Err(invalid("invalid turn byte")),
    };
    let position = Position {
        board: Board::new(player, opponent),
        side_to_move,
    };
    validate_board(&position.board)
        .map_err(|error| invalid(format!("invalid saved position: {error:?}")))?;
    if position.board.popcount() as usize + depth != goal.board.popcount() as usize {
        return Err(invalid("saved position does not belong to the stone layer"));
    }
    Ok(position)
}

pub(super) fn digest(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut block = [0; 65536];
    loop {
        let bytes = file.read(&mut block)?;
        if bytes == 0 {
            break;
        }
        hash.update(&block[..bytes]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn save_json(path: &Path, value: &Value) -> io::Result<()> {
    let temporary = path.with_extension("pending");
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(invalid_json)?)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    sync_parent(path)
}

pub(super) fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "committed file has no parent directory",
        )
    })?;
    File::open(parent)?.sync_all()
}

pub(super) fn read_json(path: &Path) -> io::Result<Value> {
    serde_json::from_reader(BufReader::new(File::open(path)?)).map_err(invalid_json)
}

fn invalid_json(error: serde_json::Error) -> io::Error {
    invalid(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;

    use serde_json::json;

    use super::*;
    use crate::search::{moves::successors, test_support::TemporaryDirectory};

    fn five_disc_positions() -> Vec<Position> {
        let mut positions: Vec<_> = successors(Position::initial())
            .into_iter()
            .map(|(position, _)| position)
            .collect();
        positions.sort_unstable();
        positions.dedup();
        assert!(positions.len() >= 4);
        positions
    }

    fn write_positions(path: &Path, context: &Value, positions: &[Position]) -> io::Result<u64> {
        let mut writer = PositionWriter::create(path, context, 0)?;
        for position in positions {
            writer.push(position)?;
        }
        writer.finish_synced()
    }

    #[test]
    fn fixed_records_have_exact_length_and_binary_search_matches_linear_scan() {
        let directory = TemporaryDirectory::new("binary-search");
        let path = directory.join("layer.bin");
        let context = json!({"query": "binary-search"});
        let candidates = five_disc_positions();
        let stored = vec![candidates[0], candidates[2]];
        assert_eq!(write_positions(&path, &context, &stored).unwrap(), 2);

        let header_length = 24 + serde_json::to_vec(&context).unwrap().len() as u64;
        assert_eq!(fs::metadata(&path).unwrap().len(), header_length + 17 * 2);
        let goal = candidates[0];
        let mut queries = candidates.clone();
        queries.push(Position {
            side_to_move: candidates[1].side_to_move.opposite(),
            ..candidates[1]
        });
        for query in queries {
            assert_eq!(
                contains_position(&path, &context, 0, goal, query).unwrap(),
                stored.contains(&query),
                "query={query:?}"
            );
        }
        assert!(contains_position(&path, &context, 0, goal, stored[0]).unwrap());
        assert!(contains_position(&path, &context, 0, goal, stored[1]).unwrap());
    }

    #[test]
    fn binary_search_handles_an_empty_file_and_rejects_a_truncated_record() {
        let directory = TemporaryDirectory::new("empty-and-truncated");
        let context = json!({"query": "empty-and-truncated"});
        let candidates = five_disc_positions();
        let empty_path = directory.join("empty.bin");
        assert_eq!(write_positions(&empty_path, &context, &[]).unwrap(), 0);
        assert!(
            !contains_position(&empty_path, &context, 0, candidates[0], candidates[0]).unwrap()
        );

        let truncated_path = directory.join("truncated.bin");
        write_positions(&truncated_path, &context, &candidates[..2]).unwrap();
        let file = OpenOptions::new()
            .write(true)
            .open(&truncated_path)
            .unwrap();
        file.set_len(file.metadata().unwrap().len() - 1).unwrap();
        let error = contains_position(&truncated_path, &context, 0, candidates[0], candidates[0])
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn writer_rejects_duplicate_and_descending_positions() {
        let directory = TemporaryDirectory::new("writer-order");
        let context = json!({"query": "writer-order"});
        let candidates = five_disc_positions();

        let mut duplicate =
            PositionWriter::create(&directory.join("duplicate.bin"), &context, 0).unwrap();
        duplicate.push(&candidates[0]).unwrap();
        let error = duplicate.push(&candidates[0]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let mut descending =
            PositionWriter::create(&directory.join("descending.bin"), &context, 0).unwrap();
        descending.push(&candidates[1]).unwrap();
        let error = descending.push(&candidates[0]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn reader_checks_the_stone_layer_and_decoded_position_order() {
        let directory = TemporaryDirectory::new("reader-validation");
        let context = json!({"query": "reader-validation"});
        let candidates = five_disc_positions();

        let wrong_layer = directory.join("wrong-layer.bin");
        write_positions(&wrong_layer, &context, &[Position::initial()]).unwrap();
        let mut reader = PositionReader::open(&wrong_layer, &context, 0, candidates[0]).unwrap();
        let error = reader.next().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let unordered = directory.join("unordered.bin");
        write_positions(&unordered, &context, &candidates[..2]).unwrap();
        let bytes = fs::read(&unordered).unwrap();
        let header_length = 24 + serde_json::to_vec(&context).unwrap().len();
        let mut corrupted = bytes;
        let records = &mut corrupted[header_length..header_length + 34];
        let (first, second) = records.split_at_mut(17);
        first.swap_with_slice(second);
        fs::write(&unordered, corrupted).unwrap();

        let mut reader = PositionReader::open(&unordered, &context, 0, candidates[0]).unwrap();
        reader.next().unwrap();
        let error = reader.next().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
