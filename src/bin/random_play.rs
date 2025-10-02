use rand::Rng;
use std::env;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process;

use othello_complexity_rs::lib::othello::{
    flip_generic, get_moves_generic, Board, Geometry, Standard6x6, Standard8x8,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoardSize {
    Size8,
    Size6,
}

impl BoardSize {
    fn parse_from_args() -> Self {
        let mut size = BoardSize::Size8;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-s" | "--size" => {
                    let Some(value) = args.next() else {
                        eprintln!("--size には 6 または 8 を指定してください");
                        process::exit(1);
                    };
                    size = match value.as_str() {
                        "6" | "6x6" => BoardSize::Size6,
                        "8" | "8x8" => BoardSize::Size8,
                        other => {
                            eprintln!("未知のサイズ指定: {} (6 または 8)", other);
                            process::exit(1);
                        }
                    };
                }
                "-h" | "--help" => {
                    println!("Usage: random_play [--size <6|8>]");
                    process::exit(0);
                }
                other => {
                    eprintln!("未知の引数: {}", other);
                    process::exit(1);
                }
            }
        }
        size
    }

    fn suffix(self) -> &'static str {
        match self {
            BoardSize::Size8 => "",
            BoardSize::Size6 => "_6x6",
        }
    }
}

/// 初期局面から nmoves 手ランダムに指した局面を返す
fn do_random_play<G: Geometry>(nmoves: i32) -> Board<G> {
    let mut rng = rand::rng();
    let mut b = Board::<G>::initial();

    for _ in 0..nmoves {
        let mut moves = get_moves_generic::<G>(b.player, b.opponent);
        if moves == 0 {
            let responses = get_moves_generic::<G>(b.opponent, b.player);
            if responses == 0 {
                continue;
            }
            b = Board::new(b.opponent, b.player);
            moves = responses;
        }

        let mut move_bits: Vec<u32> = Vec::new();
        let mut tmp = moves;
        while tmp != 0 {
            let idx = tmp.trailing_zeros();
            move_bits.push(idx);
            tmp &= tmp - 1;
        }
        if move_bits.is_empty() {
            continue;
        }

        let choice = move_bits[rng.random_range(0..move_bits.len() as u32) as usize];
        let bit_mask = 1u64 << choice;
        let Some(pos) = G::bit_to_index(bit_mask) else {
            continue;
        };

        let flipped = flip_generic::<G>(pos, b.player, b.opponent);
        if flipped == 0 {
            continue;
        }

        let move_bit = G::bit_by_index(pos);
        b = Board::new(b.opponent ^ flipped, b.player ^ (flipped | move_bit));
    }
    b
}

fn generate_samples<G: Geometry>(
    out_dir: &Path,
    suffix: &str,
    nmoves_range: std::ops::RangeInclusive<i32>,
) -> std::io::Result<()> {
    for nmoves in nmoves_range {
        let file_path = out_dir.join(format!("result{}{}.txt", nmoves, suffix));
        let mut file = File::create(&file_path)?;
        for _ in 0..50 {
            let board = do_random_play::<G>(nmoves);
            writeln!(file, "{}", board.to_string())?;
        }
    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    let board_size = BoardSize::parse_from_args();
    let out_dir = Path::new("result").join("random_play");
    if !out_dir.exists() {
        fs::create_dir_all(&out_dir)?;
    }

    match board_size {
        BoardSize::Size8 => {
            generate_samples::<Standard8x8>(&out_dir, board_size.suffix(), 20..=60)?
        }
        BoardSize::Size6 => {
            generate_samples::<Standard6x6>(&out_dir, board_size.suffix(), 20..=32)?
        }
    }

    Ok(())
}
