//! Portable checkpoint files: magic, removed-stone count u32 LE, record count
//! u64 LE, context length u32 LE, UTF-8 JSON context, then variable-size records.
//! Each record contains two u64 LE bitboards, a turn byte (0 black, 1 white),
//! an action-count byte, and that many action bytes (0..63 placement, 64 pass)
//! in backward path order. A stone layer can contain suffixes of different lengths.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::config::Cfg;
use crate::io::format_position;
use crate::othello::{validate_board, Board, Color, Position};
use crate::record::{replay, PlyAction};
use crate::search::core::SearchNode;

pub(super) const MAGIC: &[u8; 8] = b"OTHRBF02";

pub(super) fn identity(cfg: &Cfg, goal: Position, parallel: bool) -> Value {
    json!({"format": "absolute-reverse-bfs-v3", "goal": format_position(goal),
        "discs": cfg.discs, "block_size": cfg.block_size,
        "jobs": cfg.jobs, "parallel": parallel})
}

pub(super) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub(super) struct NodeWriter {
    writer: BufWriter<File>,
    depth: usize,
    count: u64,
}

impl NodeWriter {
    pub fn create(path: &Path, context: &Value, depth: usize) -> io::Result<Self> {
        if depth > 60 {
            return Err(invalid("stone depth exceeds the Othello bound"));
        }
        let data = serde_json::to_vec(context).map_err(invalid_json)?;
        let mut writer = BufWriter::new(File::create(path)?);
        writer.write_all(MAGIC)?;
        writer.write_all(&(depth as u32).to_le_bytes())?;
        writer.write_all(&0_u64.to_le_bytes())?;
        writer.write_all(&(data.len() as u32).to_le_bytes())?;
        writer.write_all(&data)?;
        Ok(Self {
            writer,
            depth,
            count: 0,
        })
    }

    pub fn push(&mut self, node: &SearchNode) -> io::Result<()> {
        if node.reverse_actions.len() > 120
            || node
                .reverse_actions
                .iter()
                .filter(|action| matches!(action, PlyAction::Play(_)))
                .count()
                != self.depth
            || node
                .reverse_actions
                .iter()
                .any(|action| matches!(action, PlyAction::Play(square) if *square >= 64))
        {
            return Err(invalid(
                "inconsistent saved action sequence for stone layer",
            ));
        }
        self.writer
            .write_all(&node.position.board.player.to_le_bytes())?;
        self.writer
            .write_all(&node.position.board.opponent.to_le_bytes())?;
        self.writer.write_all(&[match node.position.side_to_move {
            Color::Black => 0,
            Color::White => 1,
        }])?;
        self.writer.write_all(&[node.reverse_actions.len() as u8])?;
        for action in &node.reverse_actions {
            self.writer.write_all(&[match *action {
                PlyAction::Play(square) => square as u8,
                PlyAction::Pass => 64,
            }])?;
        }
        self.count += 1;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<u64> {
        self.writer.flush()?;
        let mut file = self
            .writer
            .into_inner()
            .map_err(|error| error.into_error())?;
        file.seek(SeekFrom::Start(12))?;
        file.write_all(&self.count.to_le_bytes())?;
        file.sync_all()?;
        Ok(self.count)
    }
}

pub(super) struct NodeReader {
    reader: BufReader<File>,
    depth: usize,
    remaining: u64,
    bytes_left: u64,
    goal: Position,
}

impl NodeReader {
    pub fn open(path: &Path, context: &Value, depth: usize, goal: Position) -> io::Result<Self> {
        let file = File::open(path)?;
        let bytes = file.metadata()?.len();
        let mut reader = BufReader::new(file);
        let mut magic = [0_u8; 8];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(invalid("unrecognized external BFS file header"));
        }
        let mut u32_bytes = [0; 4];
        let mut u64_bytes = [0; 8];
        reader.read_exact(&mut u32_bytes)?;
        let stored_depth = u32::from_le_bytes(u32_bytes) as usize;
        if stored_depth != depth || depth > 60 {
            return Err(invalid("checkpoint stone depth mismatch"));
        }
        reader.read_exact(&mut u64_bytes)?;
        let remaining = u64::from_le_bytes(u64_bytes);
        reader.read_exact(&mut u32_bytes)?;
        let context_length = u32::from_le_bytes(u32_bytes) as usize;
        if context_length > 1_000_000 {
            return Err(invalid("checkpoint context is too large"));
        }
        let mut data = vec![0; context_length];
        reader.read_exact(&mut data)?;
        let stored: Value = serde_json::from_slice(&data).map_err(invalid_json)?;
        if &stored != context {
            return Err(invalid("checkpoint query or search settings mismatch"));
        }
        let bytes_left = bytes
            .checked_sub(24 + context_length as u64)
            .ok_or_else(|| invalid("incomplete external BFS header"))?;
        let minimum = remaining.checked_mul(18 + depth as u64);
        let maximum = remaining.checked_mul(18 + 120);
        if minimum.is_none_or(|minimum| bytes_left < minimum)
            || maximum.is_none_or(|maximum| bytes_left > maximum)
        {
            return Err(invalid("incomplete or extra external BFS records"));
        }
        Ok(Self {
            reader,
            depth,
            remaining,
            bytes_left,
            goal,
        })
    }

    pub fn next(&mut self) -> io::Result<Option<SearchNode>> {
        if self.remaining == 0 {
            if self.bytes_left != 0 {
                return Err(invalid("extra external BFS record bytes"));
            }
            return Ok(None);
        }
        let mut bits = [0; 8];
        self.reader.read_exact(&mut bits)?;
        let player = u64::from_le_bytes(bits);
        self.reader.read_exact(&mut bits)?;
        let opponent = u64::from_le_bytes(bits);
        let mut byte = [0];
        self.reader.read_exact(&mut byte)?;
        let side_to_move = match byte[0] {
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
        if position.board.popcount() as usize + self.depth != self.goal.board.popcount() as usize {
            return Err(invalid("saved position does not belong to the stone layer"));
        }
        self.reader.read_exact(&mut byte)?;
        let action_count = byte[0] as usize;
        if action_count > 120
            || action_count < self.depth
            || self.bytes_left < 18 + action_count as u64
        {
            return Err(invalid("invalid saved action count"));
        }
        let mut reverse_actions = Vec::with_capacity(action_count);
        for _ in 0..action_count {
            self.reader.read_exact(&mut byte)?;
            reverse_actions.push(match byte[0] {
                0..=63 => PlyAction::Play(byte[0] as usize),
                64 => PlyAction::Pass,
                _ => return Err(invalid("invalid saved action")),
            });
        }
        if reverse_actions
            .iter()
            .filter(|action| matches!(action, PlyAction::Play(_)))
            .count()
            != self.depth
        {
            return Err(invalid(
                "saved placement count does not match the stone layer",
            ));
        }
        let forward_actions: Vec<_> = reverse_actions.iter().rev().copied().collect();
        let positions = replay(position, &forward_actions).map_err(invalid)?;
        if positions.last() != Some(&self.goal) {
            return Err(invalid("saved suffix does not reach the query goal"));
        }
        self.remaining -= 1;
        self.bytes_left -= 18 + action_count as u64;
        if self.remaining == 0 && self.bytes_left != 0 {
            return Err(invalid("extra external BFS record bytes"));
        }
        Ok(Some(SearchNode {
            position,
            reverse_actions,
        }))
    }
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
    fs::rename(temporary, path)
}

pub(super) fn read_json(path: &Path) -> io::Result<Value> {
    serde_json::from_reader(BufReader::new(File::open(path)?)).map_err(invalid_json)
}

fn invalid_json(error: serde_json::Error) -> io::Error {
    invalid(error.to_string())
}
