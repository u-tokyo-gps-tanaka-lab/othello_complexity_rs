use rand::rng;
use rand::rngs::ThreadRng;
use rand::Rng; // 乱数生成のため
use std::env;

use othellolib::{flip, get_moves, Board};
use std::cmp::{max, min};

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

fn mk_rand(rng: &mut ThreadRng, lim: u128) -> u128 {
    // u128 の範囲全体から乱数を生成
    let y = {
        let maxv: u128 = (u128::MAX / lim) * lim;
        let mut z = 0;
        loop {
            let x: u128 = rng.gen();
            if x < maxv {
                z = x % lim;
                break;
            }
        }
        z
    };
    y
}
fn mk_rand_board(rng: &mut ThreadRng, n: usize) -> Board {
    let mut player: u64 = 0;
    let mut opponent: u64 = 0;
    if n == 0 {
        let lim: u128 = 3_u128.pow(60) * 2_u128.pow(4);
        let mut v = mk_rand(rng, lim);
        for y in 0..8 {
            for x in 0..8 {
                let i = y * 8 + x;
                let sq = if 3 <= x && x <= 4 && 3 <= y && y <= 4 {
                    let ans = (v % 2) + 1;
                    v /= 2;
                    ans
                } else {
                    let ans = v % 3;
                    v /= 3;
                    ans
                };
                if sq == 1 {
                    player |= (1u64 << i);
                } else if sq == 2 {
                    opponent |= (1u64 << i);
                }
            }
        }
    } else {
        let mut rest_stone = n;
        let mut rest_sq = 60;
        for y in 0..8 {
            for x in 0..8 {
                let i = y * 8 + x;
                if 3 <= x && x <= 4 && 3 <= y && y <= 4 {
                    let v = mk_rand(rng, 2);
                    if v == 0 {
                        player |= (1u64 << i);
                    } else {
                        opponent |= (1u64 << i);
                    }
                } else {
                    rest_sq -= 1;
                    let mut set_count: u128 = 0;
                    let mut blank_count: u128 = 0;
                    if rest_sq < rest_stone {
                        // always set
                        set_count = 1;
                    } else if rest_stone == 0 {
                        blank_count = 1;
                    } else {
                        set_count = combination_u128(rest_sq, rest_stone - 1).unwrap();
                        blank_count = combination_u128(rest_sq, rest_stone).unwrap();
                    }
                    let v = mk_rand(rng, set_count + blank_count);
                    if v < set_count {
                        rest_stone -= 1;
                        let v = mk_rand(rng, 2);
                        if v == 0 {
                            player |= (1u64 << i);
                        } else {
                            opponent |= (1u64 << i);
                        }
                    }
                }
            }
        }
    }
    Board::new(player, opponent)
}
fn main() -> std::io::Result<()> {
    let args: Vec<String> = env::args().collect();
    // デフォルト値
    let mut n: usize = 0;
    let mut rng = rand::rng();
    // 引数を順番に走査
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--n" {
            if i + 1 < args.len() {
                n = args[i + 1]
                    .parse::<usize>()
                    .expect("整数を指定してください");
            } else {
                eprintln!("--n の後に数値を指定してください");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    for _ in 0..50 {
        let b = mk_rand_board(&mut rng, n);
        println!("{}", b.to_string());
    }
    Ok(())
}
