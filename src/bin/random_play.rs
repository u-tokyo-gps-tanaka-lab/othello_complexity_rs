use rand::Rng; // 乱数生成のため
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use othello_complexity_rs::lib::othello::{flip, get_moves, Board};

/// 初期局面から nmoves 手ランダムに指した局面を返す
fn do_random_play(nmoves: i32) -> Board {
    let mut rng = rand::rng();
    let mut b = Board::initial();

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
        b = Board {
            player: b.opponent ^ flipped,
            opponent: b.player ^ (flipped | (1u64 << idx)),
        };
    }
    b
}

fn main() -> std::io::Result<()> {
    let out_dir = Path::new("result").join("random_play");
    if !out_dir.exists() {
        fs::create_dir_all(&out_dir)?;
    }

    for nmoves in 20..=60 {
        let file_path = out_dir.join(format!("result{}.txt", nmoves));
        let mut file = File::create(&file_path)?;
        for _ in 0..50 {
            let b = do_random_play(nmoves);
            writeln!(file, "{}", b.to_string())?;
        }
    }
    Ok(())
}
