use crate::lib::othello::{Board, Geometry, Standard8x8};
use std::fs::File;
use std::io::{self, BufRead, BufReader};

/// 64セルの 'X', 'O', '-' 文字列を Board に変換。失敗したら None。
pub fn parse_line_to_board_generic<G: Geometry>(line: &str) -> Option<Board<G>> {
    let chars: Vec<char> = line
        .chars()
        .filter(|&c| c == 'X' || c == 'O' || c == '-')
        .collect();

    if chars.is_empty() {
        return None;
    }

    match chars.len() {
        len if len == G::CELL_COUNT => {
            let mut player = 0u64;
            let mut opponent = 0u64;
            for pos in 0..G::CELL_COUNT {
                let bit = G::bit_by_index(pos);
                match chars[pos] {
                    'X' => player |= bit,
                    'O' => opponent |= bit,
                    '-' => {}
                    _ => return None,
                }
            }
            Some(Board::new(player, opponent))
        }
        64 => {
            let mut player = 0u64;
            let mut opponent = 0u64;
            let region = G::region_mask();
            for (idx, ch) in chars.iter().enumerate() {
                let bit = 1u64 << idx;
                if (bit & region) == 0 {
                    if *ch != '-' {
                        return None;
                    }
                    continue;
                }
                match ch {
                    'X' => player |= bit,
                    'O' => opponent |= bit,
                    '-' => {}
                    _ => return None,
                }
            }
            Some(Board::new(player, opponent))
        }
        _ => None,
    }
}

/// 64セルの 'X', 'O', '-' 文字列を Board に変換。失敗したら None。
pub fn parse_line_to_board(line: &str) -> Option<Board> {
    parse_line_to_board_generic::<Standard8x8>(line)
}

/// ファイルから 'X', 'O', '-' 文字列を読み込み、Board の Vec に変換。失敗したら Err。
pub fn parse_file_to_boards_generic<G: Geometry>(path: &str) -> io::Result<Vec<Board<G>>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut boards: Vec<Board<G>> = Vec::new();

    for line in reader.lines() {
        let l = line?;
        let filtered: String = l
            .chars()
            .filter(|&c| c == 'X' || c == 'O' || c == '-')
            .collect();
        if let Some(b) = parse_line_to_board_generic::<G>(&filtered) {
            boards.push(b);
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

pub fn parse_file_to_boards(path: &str) -> io::Result<Vec<Board>> {
    parse_file_to_boards_generic::<Standard8x8>(path)
}
