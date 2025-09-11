use crate::lib::othello::Board;
use std::fs::File;
use std::io::{self, BufRead, BufReader};

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
