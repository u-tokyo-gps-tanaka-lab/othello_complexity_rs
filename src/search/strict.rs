use std::collections::HashSet;
use std::io;
use std::path::Path;

use crate::{
    io::{ensure_tri_outputs, parse_file_to_boards},
    othello::{get_moves, validate_board, Board},
    search::{
        forward::make_fwd_table_strict,
        parallel_gbfs::parallel_retrospective_greedy_best_first_search_strict,
    },
};

pub struct StrictLeafCache {
    searched: HashSet<[u64; 2]>,
    leaf: HashSet<[u64; 2]>,
}

impl StrictLeafCache {
    pub fn new(discs: i32) -> Self {
        let mut searched = HashSet::new();
        let mut leaf = HashSet::new();
        strict_fwd_search(&Board::initial(), &mut searched, &mut leaf, discs);
        Self { searched, leaf }
    }

    pub fn searched_count(&self) -> usize {
        self.searched.len()
    }

    pub fn leaf_count(&self) -> usize {
        self.leaf.len()
    }

    pub fn leaf(&self) -> &HashSet<[u64; 2]> {
        &self.leaf
    }

    pub fn sorted_leaf(&self) -> Vec<[u64; 2]> {
        let mut leaf = self.leaf.iter().copied().collect::<Vec<_>>();
        leaf.sort_unstable();
        leaf
    }
}

pub fn strict_fwd_search(
    board: &Board,
    searched: &mut HashSet<[u64; 2]>,
    leafnode: &mut HashSet<[u64; 2]>,
    discs: i32,
) {
    let key = [board.player, board.opponent];

    if board.popcount() >= discs as u32 {
        if get_moves(board.player, board.opponent) != 0 {
            leafnode.insert(key);
            return;
        } else if get_moves(board.opponent, board.player) != 0 {
            strict_fwd_search(&board.swapped(), searched, leafnode, discs);
        }
        return;
    }

    if !searched.insert(key) {
        return;
    }

    let mut moves = get_moves(board.player, board.opponent);
    if moves == 0 {
        if get_moves(board.opponent, board.player) != 0 {
            strict_fwd_search(&board.swapped(), searched, leafnode, discs);
        }
        return;
    }

    while moves != 0 {
        let idx = moves.trailing_zeros() as usize;
        moves &= moves - 1;

        let flipped = crate::othello::flip(idx, board.player, board.opponent);
        if flipped == 0 {
            continue;
        }
        let next = Board::new(
            board.opponent ^ flipped,
            board.player ^ (flipped | (1_u64 << idx)),
        );
        strict_fwd_search(&next, searched, leafnode, discs);
    }
}

pub fn run_strict_parallel_gbfs(
    input: &Path,
    out_dir: &Path,
    discs: i32,
    node_limit: usize,
    use_lp: bool,
    rayon_threads: Option<usize>,
) -> io::Result<()> {
    let boards = parse_file_to_boards(&input.to_string_lossy())?;
    let total_input = boards.len();
    println!(
        "info: read {} board(s) from '{}'.",
        total_input,
        input.display()
    );

    let mut outputs = ensure_tri_outputs(out_dir, "reverse_strict_gbfs")?;
    println!("info: writing outputs under '{}'", out_dir.display());

    let rayon_threads = match rayon_threads {
        Some(n) if n > 0 => n,
        Some(_) => 1,
        None => std::thread::available_parallelism()
            .map(|n| n.get().min(64))
            .unwrap_or(1),
    };

    for board in boards {
        let line = board.to_string();

        if validate_board(&board).is_err() {
            outputs.write_ng(&line)?;
            continue;
        }

        let leaf = make_fwd_table_strict(&[board.player, board.opponent], discs);
        println!("info: strict target-specific leaf = {}", leaf.len());

        let result = parallel_retrospective_greedy_best_first_search_strict(
            &board,
            discs,
            &leaf,
            node_limit,
            use_lp,
            rayon_threads,
        );
        outputs.write_result(&result, &line)?;
        outputs.flush()?;
    }

    outputs.flush()
}
