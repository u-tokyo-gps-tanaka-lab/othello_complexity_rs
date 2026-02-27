use crate::othello::Board;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

/// 3分類 (OK/NG/UNKNOWN) を表す列挙型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriCategory {
    Ok,
    Ng,
    Unknown,
}

/// 結果を OK/NG/UNKNOWN に振り分けるトレイト
pub trait TriOutcome {
    fn category(&self) -> TriCategory;
}

/// 64セルの 'X', 'O', '-' 文字列を Board に変換。失敗したら None。
pub fn parse_line_to_board(line: &str) -> Option<Board> {
    let mut player: u64 = 0;
    let mut opponent: u64 = 0;
    let mut idx = 0u32;
    for c in line.chars() {
        match c {
            'X' => {
                if idx >= 64 {
                    return None;
                }
                player |= 1_u64 << idx;
                idx += 1;
            }
            'O' => {
                if idx >= 64 {
                    return None;
                }
                opponent |= 1_u64 << idx;
                idx += 1;
            }
            '-' => {
                if idx >= 64 {
                    return None;
                }
                idx += 1;
            }
            _ => (),
        }
    }

    if idx == 64 {
        Some(Board::new(player, opponent))
    } else {
        None
    }
}

/// ファイルから 'X', 'O', '-' 文字列を読み込み、Board の Vec に変換。失敗したら Err。
pub fn parse_file_to_boards(path: &str) -> io::Result<Vec<Board>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut boards: Vec<Board> = Vec::new();

    for line in reader.lines() {
        let l = line?;
        let filtered: String = l
            .chars()
            .filter(|&c| c == 'X' || c == 'O' || c == '-')
            .collect();
        if filtered.len() == 64 {
            if let Some(b) = parse_line_to_board(&filtered) {
                boards.push(b);
            }
        }
    }

    if !boards.is_empty() {
        return Ok(boards);
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "failed to parse any 64-cell X/O/- board(s)",
    ))
}

/// 出力ディレクトリを作成し、TriOutputsを返す
pub fn ensure_tri_outputs(out_dir: &Path, prefix: &str) -> io::Result<TriOutputs> {
    fs::create_dir_all(out_dir)?;
    TriOutputs::new(out_dir, prefix)
}

/// 結果を3つのファイル（OK/NG/UNKNOWN）に書き出すための汎用構造体
pub struct TriOutputs {
    pub ok: io::BufWriter<File>,
    pub ng: io::BufWriter<File>,
    pub unknown: io::BufWriter<File>,
}

impl TriOutputs {
    fn new(out_dir: &Path, prefix: &str) -> io::Result<Self> {
        let ok = io::BufWriter::new(File::create(out_dir.join(format!("{prefix}_OK.txt")))?);
        let ng = io::BufWriter::new(File::create(out_dir.join(format!("{prefix}_NG.txt")))?);
        let unknown =
            io::BufWriter::new(File::create(out_dir.join(format!("{prefix}_UNKNOWN.txt")))?);
        Ok(TriOutputs { ok, ng, unknown })
    }

    pub fn write_ok(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.ok, "{}", line)
    }

    pub fn write_ng(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.ng, "{}", line)
    }

    pub fn write_unknown(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.unknown, "{}", line)
    }

    pub fn write_result(&mut self, result: &impl TriOutcome, line: &str) -> io::Result<()> {
        match result.category() {
            TriCategory::Ok => self.write_ok(line),
            TriCategory::Ng => self.write_ng(line),
            TriCategory::Unknown => self.write_unknown(line),
        }
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.ok.flush()?;
        self.ng.flush()?;
        self.unknown.flush()?;
        Ok(())
    }
}
