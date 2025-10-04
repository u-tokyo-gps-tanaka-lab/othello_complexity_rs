use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::lib::io::parse_file_to_boards;
use crate::lib::othello::{Board, CENTER_MASK};
use crate::lib::search::{search, SearchResult};

pub struct BasicArgs {
    pub input: PathBuf,
    pub out_dir: PathBuf,
}

pub fn parse_basic_cli() -> io::Result<BasicArgs> {
    let mut input: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--out-dir" => {
                let value = args.next().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "missing value for --out-dir")
                })?;
                out_dir = Some(PathBuf::from(value));
            }
            _ => {
                if arg.starts_with('-') {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("unknown flag: {}", arg),
                    ));
                }
                if input.is_none() {
                    input = Some(PathBuf::from(arg));
                } else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("unexpected extra argument: {}", arg),
                    ));
                }
            }
        }
    }

    let input = input.unwrap_or_else(|| PathBuf::from("board.txt"));
    let out_dir =
        out_dir.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result"));

    Ok(BasicArgs { input, out_dir })
}

pub fn read_boards(path: &Path) -> io::Result<Vec<Board>> {
    parse_file_to_boards(&path.to_string_lossy())
}

pub fn ensure_outputs(out_dir: &Path) -> io::Result<ReverseOutputs> {
    fs::create_dir_all(out_dir)?;
    ReverseOutputs::create(out_dir)
}

pub fn read_env_with_default<T>(key: &str, default: T) -> T
where
    T: FromStr,
{
    env::var(key)
        .ok()
        .and_then(|s| s.parse::<T>().ok())
        .unwrap_or(default)
}

pub struct LeafCache {
    searched: HashSet<[u64; 2]>,
    leaf: HashSet<[u64; 2]>,
}

impl LeafCache {
    pub fn new(discs: i32) -> Self {
        let mut searched: HashSet<[u64; 2]> = HashSet::new();
        let mut leafnode: HashSet<[u64; 2]> = HashSet::new();
        let initial = Board::initial();
        search(&initial, &mut searched, &mut leafnode, discs);
        LeafCache {
            searched,
            leaf: leafnode,
        }
    }

    pub fn searched_count(&self) -> usize {
        self.searched.len()
    }

    pub fn leaf_count(&self) -> usize {
        self.leaf.len()
    }

    pub fn leaf(&self) -> &HashSet<[u64; 2]> {
        &self.leaf
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardValidation {
    Overlap,
    MissingCenter,
}

pub fn validate_board(board: &Board) -> Result<(), BoardValidation> {
    if (board.player & board.opponent) != 0 {
        return Err(BoardValidation::Overlap);
    }
    let occupied = board.player | board.opponent;
    if (occupied & CENTER_MASK) != CENTER_MASK {
        return Err(BoardValidation::MissingCenter);
    }
    Ok(())
}

pub struct ReverseOutputs {
    pub ok: io::BufWriter<File>,
    pub ng: io::BufWriter<File>,
    pub unknown: io::BufWriter<File>,
}

impl ReverseOutputs {
    fn create(out_dir: &Path) -> io::Result<Self> {
        let ok = io::BufWriter::new(File::create(out_dir.join("reverse_OK.txt"))?);
        let ng = io::BufWriter::new(File::create(out_dir.join("reverse_NG.txt"))?);
        let unknown = io::BufWriter::new(File::create(out_dir.join("reverse_UNKNOWN.txt"))?);
        Ok(ReverseOutputs { ok, ng, unknown })
    }

    pub fn write_result(&mut self, result: SearchResult, line: &str) -> io::Result<()> {
        match result {
            SearchResult::Found => writeln!(self.ok, "{}", line),
            SearchResult::NotFound => writeln!(self.ng, "{}", line),
            SearchResult::Unknown => writeln!(self.unknown, "{}", line),
        }
    }

    pub fn write_invalid(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.ng, "{}", line)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.ok.flush()?;
        self.ng.flush()?;
        self.unknown.flush()?;
        Ok(())
    }
}
