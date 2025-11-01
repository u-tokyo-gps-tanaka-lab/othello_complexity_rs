use crate::{othello::Board, search::core::SearchResult};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

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

/// 出力ディレクトリを作成し、ReverseOutputsを返す
pub fn ensure_outputs(out_dir: &Path) -> io::Result<ReverseOutputs> {
    fs::create_dir_all(out_dir)?;
    ReverseOutputs::create(out_dir)
}

/// reverse探索の結果を3つのファイル（OK/NG/UNKNOWN）に書き出すための構造体
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
