use rand::Rng; // 乱数生成のため
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use othello_complexity_rs::othello::{flip, get_moves, Board};

/// ビット位置 (0-63) を先手視点の座標文字列 (a1-h8) に変換
#[inline]
fn idx_to_coord(idx: usize) -> String {
    let x = idx % 8;
    let y = idx / 8;
    let file = (b'a' + x as u8) as char;
    format!("{}{}", file, y + 1)
}

/// 初期局面から nmoves 手ランダムに指した局面とその棋譜を返す
fn do_random_play(nmoves: i32) -> (Board, String) {
    let mut rng = rand::rng();
    let mut b = Board::initial();
    let mut record = String::new();

    for _ in 0..nmoves {
        let mut m = get_moves(b.player, b.opponent);
        if m == 0 {
            let m1 = get_moves(b.opponent, b.player);
            if m1 == 0 {
                continue;
            }
            b = Board::new(b.opponent, b.player);
            m = m1;
        }
        let cnt = m.count_ones();
        let r = rng.random_range(0..cnt);
        let mut idx = 0;
        for _ in 0..=r {
            idx = m.trailing_zeros();
            m &= m - 1;
        }
        let flipped = flip(idx as usize, b.player, b.opponent);
        if flipped == 0 {
            continue;
        }
        record.push_str(&idx_to_coord(idx as usize));
        b = Board {
            player: b.opponent ^ flipped,
            opponent: b.player ^ (flipped | (1u64 << idx)),
        };
    }
    (b, record)
}

fn main() -> std::io::Result<()> {
    let out_dir = Path::new("result").join("random_play");
    if !out_dir.exists() {
        fs::create_dir_all(&out_dir)?;
    }

    for nmoves in 20..=60 {
        let position_path = out_dir.join(format!("result{}.txt", nmoves));
        let record_path = out_dir.join(format!("result{}_record.txt", nmoves));
        let mut position_file = File::create(&position_path)?;
        let mut record_file = File::create(&record_path)?;
        for _ in 0..50 {
            let (b, record) = do_random_play(nmoves);
            writeln!(position_file, "{}", b.to_string())?;
            writeln!(record_file, "{record}")?;
        }
    }
    Ok(())
}
