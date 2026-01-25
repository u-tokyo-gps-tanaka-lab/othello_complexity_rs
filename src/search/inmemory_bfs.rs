use dashmap::DashSet;
use rayon::ThreadPoolBuilder;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use crate::othello::{get_moves, Board};
use crate::prunings::impossible_edges::check_edge_patterns;
use crate::prunings::linear_programming::check_lp;
use crate::prunings::occupancy::check_occupancy;
use crate::prunings::seg3::check_seg3_more;
use crate::search::core::{prev_states_with_buffer, SearchResult};
use crate::search::forward::is_leaf;

/// Parallel in-memory retrospective BFS search
///
/// Searches backward from the goal board state to find if a path exists
/// from the initial Othello position.
///
/// # Parameters
/// - `board`: The start node
/// - `discs`: Target disc count for meeting forward search leaf nodes
/// - `leafnode`: Sorted vector of canonical leaf nodes from forward search
/// - `use_lp`: Enable LP-solver pruning
///
/// # Returns
/// - `SearchResult::Found`: A path exists from initial state to goal
/// - `SearchResult::NotFound`: No path exists (search exhausted or tree unexpandable)
pub fn parallel_inmemory_retrospective_bfs(
    board: &Board,
    discs: i32,
    leafnode: &Vec<[u64; 2]>,
    use_lp: bool,
) -> SearchResult {
    let initial_disc_count = board.popcount() as i32;

    // transposition table
    let visited: Arc<DashSet<[u64; 2]>> = Arc::new(DashSet::new());

    // 早期return
    if initial_disc_count <= discs {
        let unique = board.unique();
        let mut current_layer = vec![unique];
        visited.insert(unique);
        expand_pass_nodes(&mut current_layer, &visited, use_lp);
        for state in &current_layer {
            if is_leaf(*state, leafnode, discs) {
                return SearchResult::Found;
            }
        }
        return SearchResult::NotFound;
    }

    // 初期ノードを登録
    let mut starts = vec![[board.player, board.opponent]];
    if get_moves(board.opponent, board.player) == 0 {
        starts.push([board.opponent, board.player]);
    }

    let mut current_layer: Vec<[u64; 2]> = vec![];
    for s in starts {
        let unique = Board::new(s[0], s[1]).unique();
        if visited.insert(unique) {
            current_layer.push(unique);
        }
    }

    // スレッドプール構築
    let num_threads = thread::available_parallelism()
        .map(|n| n.get().min(64))
        .unwrap_or(1);
    let pool = ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .thread_name(|i| format!("bfs-worker-{i}"))
        .build()
        .expect("failed to build thread pool");

    for i in (discs..initial_disc_count).rev() {
        if current_layer.is_empty() {
            return SearchResult::NotFound;
        }

        // 直前手がパスだった場合の対応
        expand_pass_nodes(&mut current_layer, &visited, use_lp);

        let next_layer: Arc<Mutex<Vec<[u64; 2]>>> = Arc::new(Mutex::new(Vec::new()));
        let found: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        // Parallel expansion of current layer
        pool.scope(|scope| {
            let chunk_size = (current_layer.len() / num_threads).max(1);

            for chunk in current_layer.chunks(chunk_size) {
                let visited = Arc::clone(&visited);
                let next_layer = Arc::clone(&next_layer);
                let found = Arc::clone(&found);

                scope.spawn(move |_| {
                    let mut local_next: Vec<[u64; 2]> = Vec::with_capacity(chunk.len() * 8);
                    expand_nodes(
                        chunk,
                        &visited,
                        &mut local_next,
                        &found,
                        discs,
                        leafnode,
                        use_lp,
                    );
                    if !local_next.is_empty() {
                        next_layer.lock().unwrap().extend(local_next);
                    }
                });
            }
        });

        println!("layer={} : {}", i, current_layer.len());

        if found.load(Ordering::Acquire) {
            return SearchResult::Found;
        }

        current_layer = std::mem::take(&mut *next_layer.lock().unwrap());
    }

    expand_pass_nodes(&mut current_layer, &visited, use_lp);

    for state in &current_layer {
        if is_leaf(*state, leafnode, discs) {
            return SearchResult::Found;
        }
    }

    SearchResult::NotFound
}

/// Expand a chunk of states in parallel
fn expand_nodes(
    chunk: &[[u64; 2]],
    visited: &DashSet<[u64; 2]>,
    next_layer: &mut Vec<[u64; 2]>,
    found: &AtomicBool,
    final_target_disc: i32,
    leafnode: &Vec<[u64; 2]>,
    use_lp: bool,
) {
    // Thread-local buffer for retrospective_flip results
    let mut retroflips = [0u64; 10_000];
    let mut prev_buf: Vec<[u64; 2]> = Vec::with_capacity(256);

    for &node in chunk {
        if found.load(Ordering::Relaxed) {
            return;
        }

        prev_states_with_buffer(node, &mut retroflips, &mut prev_buf);
        for prev in prev_buf.iter().copied() {
            if found.load(Ordering::Relaxed) {
                return;
            }
            // 枝刈り
            if !check_edge_patterns(prev[0], prev[1])
                || !check_occupancy(prev[0] | prev[1])
                || !check_seg3_more(prev[0], prev[1])
                || (use_lp && !check_lp(prev[0], prev[1], false))
            {
                continue;
            }

            let unique = Board::new(prev[0], prev[1]).unique();

            if !visited.insert(unique) {
                continue; // Already visited
            }

            // 目標判定
            let disc_count = (unique[0] | unique[1]).count_ones() as i32;
            if disc_count == final_target_disc {
                if is_leaf(unique, leafnode, final_target_disc) {
                    found.store(true, Ordering::Release);
                    return;
                }
                // Don't add to next_layer if at final target disc count but not a match
                continue;
            }

            // Add to next layer for further expansion
            next_layer.push(unique);
        }
    }
}

/// 直前手がパスだった場合の対応
fn expand_pass_nodes(layer: &mut Vec<[u64; 2]>, visited: &DashSet<[u64; 2]>, use_lp: bool) {
    let mut idx = 0;
    while idx < layer.len() {
        let node = layer[idx];
        // 前局面がパスなら、盤面は同じで手番だけ入れ替わる
        if get_moves(node[1], node[0]) == 0 {
            let swapped = [node[1], node[0]];
            if !check_edge_patterns(swapped[0], swapped[1])
                || !check_occupancy(swapped[0] | swapped[1])
                || !check_seg3_more(swapped[0], swapped[1])
                || (use_lp && !check_lp(swapped[0], swapped[1], false))
            {
                idx += 1;
                continue;
            }

            let pass = Board::new(swapped[0], swapped[1]).unique();
            if visited.insert(pass) {
                layer.push(pass);
            }
        }
        idx += 1;
    }
}
