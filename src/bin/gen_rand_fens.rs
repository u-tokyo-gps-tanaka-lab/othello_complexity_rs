use rand::rngs::ThreadRng;
use rand::Rng;
use std::cmp::min;
use std::env;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process;

use othello_complexity_rs::lib::othello::{Board, Geometry, Standard6x6, Standard8x8};

/// nCk を u128 で返す。u128 を超える場合は None。
pub fn combination_u128(n: usize, k: usize) -> Option<u128> {
    if k > n {
        return Some(0); // 慣習的に n < k なら 0
    }
    let k = min(k, n - k);
    if k == 0 {
        return Some(1);
    }

    let mut res: u128 = 1;

    for i in 1..=k {
        // 分子 (n - k + i), 分母 i
        let mut a = (n - k + i) as u128;
        let mut b = i as u128;

        // 分子と分母でまず約分
        let g1 = gcd_u128(a, b);
        a /= g1;
        b /= g1;

        // さらに現在の res と分母 b を約分（分母をできるだけ 1 に近づける）
        let g2 = gcd_u128(res, b);
        res /= g2;
        b /= g2;

        // ここまでで b は通常 1 になる（ならなくても整数結果は保たれる）
        // まず掛け算でオーバーフロー検出
        res = res.checked_mul(a)?;
        if b != 1 {
            // 念のため（整数性は保たれているはず）
            debug_assert!(res % b == 0);
            res /= b;
        }
    }
    Some(res)
}

#[inline]
fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoardSize {
    Size8,
    Size6,
}

impl BoardSize {
    fn parse_from_args() -> (Self, usize, usize) {
        let mut size = BoardSize::Size8;
        let mut stone_count: usize = 0;
        let mut gen_count: usize = 50;

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-n" => {
                    let Some(value) = args.next() else {
                        eprintln!("-n の後に数値を指定してください");
                        process::exit(1);
                    };
                    stone_count = value.parse::<usize>().unwrap_or_else(|_| {
                        eprintln!("-n には整数を指定してください");
                        process::exit(1);
                    });
                }
                "-c" => {
                    let Some(value) = args.next() else {
                        eprintln!("-c の後に数値を指定してください");
                        process::exit(1);
                    };
                    gen_count = value.parse::<usize>().unwrap_or_else(|_| {
                        eprintln!("-c には整数を指定してください");
                        process::exit(1);
                    });
                    if gen_count == 0 {
                        eprintln!("-c の値は 1 以上を指定してください");
                        process::exit(1);
                    }
                }
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
                    println!("Usage: gen_rand_fens [-n stones] [-c count] [--size <6|8>]");
                    process::exit(0);
                }
                other => {
                    eprintln!("未知の引数: {}", other);
                    process::exit(1);
                }
            }
        }

        (size, stone_count, gen_count)
    }

    fn filename_prefix(self) -> &'static str {
        match self {
            BoardSize::Size8 => "result",
            BoardSize::Size6 => "result_6x6",
        }
    }
}

/// 区間 0..lim から乱数を生成
fn mk_rand(rng: &mut ThreadRng, lim: u128) -> u128 {
    let maxv: u128 = (u128::MAX / lim) * lim; // u128::MAX以下で最大のlimの倍数

    // 乱数の範囲を [0, maxv) に制限し, [maxv, u128::MAX] の値を棄却する
    loop {
        let x: u128 = rng.random();
        if x < maxv {
            return x % lim;
        }
    }
}

/// n+4マス埋まりのランダムなビットボードを生成（到達可能とは限らない）
/// - rng: 疑似乱数生成器
/// - n: 中心4マス以外に石を置くマス数 (n==0ならばマス数を限定しない全状態から抽出)
fn mk_rand_board<G: Geometry>(rng: &mut ThreadRng, n: usize) -> Board<G> {
    let mut player: u64 = 0;
    let mut opponent: u64 = 0;
    let center_mask = G::center_mask();
    let non_center_total = G::CELL_COUNT - 4;

    if n == 0 {
        let lim: u128 = 3_u128.pow(non_center_total as u32) * 2_u128.pow(4);
        let mut v = mk_rand(rng, lim);
        for y in 0..G::HEIGHT {
            for x in 0..G::WIDTH {
                let bit = G::bit_at(x, y);
                let sq = if center_mask & bit != 0 {
                    let ans = (v % 2) + 1;
                    v /= 2;
                    ans
                } else {
                    let ans = v % 3;
                    v /= 3;
                    ans
                };
                if sq == 1 {
                    player |= bit;
                } else if sq == 2 {
                    opponent |= bit;
                }
            }
        }
    } else {
        let mut rest_stone = n; //置くべき石が残りいくつあるか
        let mut rest_sq = non_center_total; //まだ石を置いていないマスの数

        for y in 0..G::HEIGHT {
            for x in 0..G::WIDTH {
                let bit = G::bit_at(x, y);
                if center_mask & bit != 0 {
                    let v = mk_rand(rng, 2);
                    if v == 0 {
                        player |= bit;
                    } else {
                        opponent |= bit;
                    }
                } else {
                    rest_sq -= 1;
                    let (set_count, blank_count): (u128, u128) = if rest_sq < rest_stone {
                        (1, 0)
                    } else if rest_stone == 0 {
                        (0, 1)
                    } else {
                        (
                            combination_u128(rest_sq, rest_stone - 1).unwrap(),
                            combination_u128(rest_sq, rest_stone).unwrap(),
                        )
                    };
                    let v = mk_rand(rng, set_count + blank_count);
                    if v < set_count {
                        rest_stone -= 1;
                        let v = mk_rand(rng, 2);
                        if v == 0 {
                            player |= bit;
                        } else {
                            opponent |= bit;
                        }
                    }
                }
            }
        }
    }
    Board::new(player, opponent)
}

fn generate_fens<G: Geometry>(
    rng: &mut ThreadRng,
    stone_count: usize,
    gen_count: usize,
    out_path: &Path,
) -> std::io::Result<()> {
    let max_stones = G::CELL_COUNT - 4;
    if stone_count > max_stones {
        eprintln!(
            "中心4マス以外に置ける石数は最大 {} 個です (指定={})",
            max_stones, stone_count
        );
        process::exit(1);
    }

    let mut file = File::create(out_path)?;
    for _ in 0..gen_count {
        let board = mk_rand_board::<G>(rng, stone_count);
        writeln!(file, "{}", board.to_string())?;
    }
    Ok(())
}

/// 実行方法: cargo run --bin gen_rand_fens -- -n {{数値}} [-c {{生成個数}}]
/// - -n {{数値}}: 中心4マス以外に石を置くマス数 (0ならばマス数を限定しない全状態から抽出)
/// - -c {{生成個数}}: 生成個数 (デフォルト50)
/// - --size <6|8>: 盤サイズ (デフォルト 8x8)
fn main() -> std::io::Result<()> {
    let (board_size, stone_count, gen_count) = BoardSize::parse_from_args();
    let mut rng = rand::rng();

    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("result")
        .join("random_board");
    if !out_dir.exists() {
        fs::create_dir_all(&out_dir)?;
    }

    let file_path = out_dir.join(format!(
        "{}_n{}_c{}.txt",
        board_size.filename_prefix(),
        stone_count,
        gen_count
    ));

    match board_size {
        BoardSize::Size8 => {
            generate_fens::<Standard8x8>(&mut rng, stone_count, gen_count, &file_path)
        }
        BoardSize::Size6 => {
            generate_fens::<Standard6x6>(&mut rng, stone_count, gen_count, &file_path)
        }
    }
}
