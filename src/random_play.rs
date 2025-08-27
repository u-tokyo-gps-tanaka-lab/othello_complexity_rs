use rand::thread_rng;
use rand::Rng; // 乱数生成のため
use std::fs;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use ::othello_complexity_rs::{flip, get_moves, Board};
fn main() -> std::io::Result<()> {
    let path = Path::new("results");
    let mut rng = rand::thread_rng();

    if !path.exists() {
        fs::create_dir_all(&path)?;
    }
    for nmoves in 20..=60 {
        let filename = format!("results/result{}.txt", nmoves);
        let mut file = File::create(&filename)?;
        for i in 0..50 {
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
                let r = rng.gen_range(0..cnt);
                let mut m1 = m;
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
            writeln!(file, "{}", b.to_string())?;
        }
    }
    Ok(())
}
