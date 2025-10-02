use std::env;
use std::process;

use othello_complexity_rs::lib::othello::{flip, get_moves6, Board6};

/// 単純なPerft(節点数数え上げ)を計算する。
fn perft(board: Board6, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }

    let moves = get_moves6(board.player, board.opponent);
    if moves == 0 {
        let opp_moves = get_moves6(board.opponent, board.player);
        if opp_moves == 0 {
            // 双方打てないので終局
            return 1;
        }
        // パス扱いで手番だけを入れ替えて探索を継続
        return perft(Board6::new(board.opponent, board.player), depth - 1);
    }

    let mut total = 0u64;
    let mut move_bits = moves;

    while move_bits != 0 {
        let idx = move_bits.trailing_zeros() as usize;
        move_bits &= move_bits - 1;

        let flipped = flip(idx, board.player, board.opponent);
        if flipped == 0 {
            continue;
        }

        let next_board = Board6::new(
            board.opponent ^ flipped,
            board.player ^ (flipped | (1u64 << idx)),
        );

        total += perft(next_board, depth - 1);
    }

    total
}

fn print_usage(program: &str) {
    eprintln!("Usage: {} <depth>", program);
}

fn main() {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "perft".to_string());

    let depth_arg = match args.next() {
        Some(arg) => arg,
        None => {
            print_usage(&program);
            process::exit(1);
        }
    };

    let depth: u32 = match depth_arg.parse() {
        Ok(d) => d,
        Err(_) => {
            eprintln!("Depth must be a non-negative integer");
            process::exit(1);
        }
    };

    let board = Board6::initial();

    for d in 1..=depth {
        let nodes = perft(board, d);
        println!("perft({}) = {}", d, nodes);
    }
}
