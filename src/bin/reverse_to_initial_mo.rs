use std::io;

use othello_complexity_rs::lib::reverse_common::{
    ensure_outputs, parse_basic_cli, read_boards, read_env_with_default, validate_board, LeafCache,
};
use othello_complexity_rs::lib::search::{retrospective_search_move_ordering, Btable};

fn run() -> io::Result<()> {
    let cli = parse_basic_cli()?;
    let boards = read_boards(&cli.input)?;
    let total_input = boards.len();
    println!(
        "info: read {} board(s) from '{}'.",
        total_input,
        cli.input.display()
    );

    let mut outputs = ensure_outputs(&cli.out_dir)?;
    println!("info: writing outputs under '{}'", cli.out_dir.display());

    let discs: i32 = read_env_with_default("DISCS", 10);
    let leaf_cache = LeafCache::new(discs);
    println!(
        "info: discs = {}: internal = {}, leaf = {}",
        discs,
        leaf_cache.searched_count(),
        leaf_cache.leaf_count()
    );

    let mut retrospective_searched: Btable = Btable::new(0x100000000, 0x10000);
    let mut retroflips: Vec<[u64; 10_000]> = vec![];

    let node_limit: usize = read_env_with_default("MAX_NODES", 1_000_000usize);
    println!("info: MAX_NODES = {}", node_limit);
    for board in boards {
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_invalid(&line)?;
            continue;
        }

        retrospective_searched.clear();
        let mut node_count: usize = 0;

        let result = retrospective_search_move_ordering(
            &board,
            false,
            discs,
            leaf_cache.leaf(),
            &mut retrospective_searched,
            &mut retroflips,
            &mut node_count,
            node_limit,
        );
        outputs.write_result(result, &line)?;
        eprintln!(
            "retrospective_searched.len() = {}",
            retrospective_searched.len()
        );
    }

    outputs.flush()
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
