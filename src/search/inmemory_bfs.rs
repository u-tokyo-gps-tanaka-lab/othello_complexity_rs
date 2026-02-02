use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use dashmap::DashSet;
use rayon::{prelude::*, ThreadPoolBuilder};

use crate::othello::{get_moves, Board};
use crate::prunings::impossible_edges::check_edge_patterns;
use crate::prunings::kissat::is_sat_ok;
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
/// - `time_limit`: Per-board time limit (None = no limit)
///
/// # Returns
/// - `SearchResult::Found`: A path exists from initial state to goal
/// - `SearchResult::NotFound`: No path exists (search exhausted or tree unexpandable)
pub fn parallel_inmemory_retrospective_bfs(
    board: &Board,
    discs: i32,
    leafnode: &Vec<[u64; 2]>,
    use_lp: bool,
    use_sat: bool,
    use_occupancy_cache: bool,
) -> SearchResult {
    let initial_disc_count = board.popcount() as i32;
    println!("target_discs={}", initial_disc_count);

    let found: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    // 早期return
    if initial_disc_count <= discs {
        let unique = board.unique();
        if is_leaf(unique, leafnode, discs) {
            return SearchResult::Found;
        }
        return SearchResult::NotFound;
    }

    // 初期ノードを登録
    let mut current_layer = vec![];
    let start_seen = DashSet::new();
    let starts = if get_moves(board.opponent, board.player) == 0 {
        vec![
            [board.player, board.opponent],
            [board.opponent, board.player],
        ]
    } else {
        vec![[board.player, board.opponent]]
    };
    for s in starts {
        let unique = Board::new(s[0], s[1]).unique();
        if start_seen.insert(unique) {
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

        // Per-layer seen set to deduplicate this expansion only
        let layer_seen: Arc<DashSet<[u64; 2]>> = Arc::new(DashSet::new());

        // Parallel expansion with thread-local aggregation
        let next_layer = pool.install(|| {
            current_layer
                .par_iter()
                .fold(ThreadLocal::new, |mut tls, &node| {
                    if found.load(Ordering::Relaxed) {
                        return tls;
                    }
                    expand_node(
                        node,
                        &layer_seen,
                        &mut tls.next,
                        &mut *tls.retroflips,
                        &mut tls.prev_buf,
                        &found,
                        discs,
                        leafnode,
                        use_lp,
                        use_sat,
                        use_occupancy_cache,
                    );
                    tls
                })
                .reduce(ThreadLocal::new, |mut a, mut b| {
                    a.next.append(&mut b.next);
                    a
                })
                .next
        });

        // let order_stats = crate::prunings::occupancy::occupancy_order_cache_stats();
        // eprintln!(
        //     "occupancy_order TT hit rate: {:.2}% (hits={}, lookups={}, misses={}, entries={})",
        //     order_stats.hit_rate() * 100.0,
        //     order_stats.hits,
        //     order_stats.lookups,
        //     order_stats.misses(),
        //     order_stats.entries,
        // );

        println!("layer={} : {}", i, next_layer.len());

        if found.load(Ordering::Acquire) {
            return SearchResult::Found;
        }

        current_layer = next_layer;
    }

    for state in &current_layer {
        if is_leaf(*state, leafnode, discs) {
            return SearchResult::Found;
        }
        if get_moves(state[1], state[0]) == 0 {
            let pass = Board::new(state[1], state[0]).unique();
            if is_leaf(pass, leafnode, discs) {
                return SearchResult::Found;
            }
        }
    }

    SearchResult::NotFound
}

struct ThreadLocal {
    next: Vec<[u64; 2]>,
    retroflips: Box<[u64; 10_000]>,
    prev_buf: Vec<[u64; 2]>,
}

impl ThreadLocal {
    fn new() -> Self {
        Self {
            next: Vec::new(),
            retroflips: Box::new([0u64; 10_000]),
            prev_buf: Vec::with_capacity(256),
        }
    }
}

/// 1手前のノードを展開する
fn expand_node(
    node: [u64; 2],
    layer_seen: &DashSet<[u64; 2]>,
    next_layer: &mut Vec<[u64; 2]>,
    retroflips: &mut [u64; 10_000],
    prev_buf: &mut Vec<[u64; 2]>,
    found: &AtomicBool,
    final_target_disc: i32,
    leafnode: &Vec<[u64; 2]>,
    use_lp: bool,
    use_sat: bool,
    use_occupancy_cache: bool,
) {
    if found.load(Ordering::Relaxed) {
        return;
    }

    expand_prev_states(
        node,
        layer_seen,
        next_layer,
        retroflips,
        prev_buf,
        found,
        final_target_disc,
        leafnode,
        use_lp,
        use_sat,
        use_occupancy_cache,
    );

    if found.load(Ordering::Relaxed) {
        return;
    }

    // 直前手がパスだった場合は、そのまま同じ石数で展開する
    if get_moves(node[1], node[0]) == 0 {
        let pass = Board::new(node[1], node[0]).unique();
        if !layer_seen.insert(pass) {
            return;
        }

        let disc_count = (pass[0] | pass[1]).count_ones() as i32;
        if disc_count == final_target_disc {
            if is_leaf(pass, leafnode, final_target_disc) {
                found.store(true, Ordering::Release);
            }
            return;
        }

        expand_prev_states(
            pass,
            layer_seen,
            next_layer,
            retroflips,
            prev_buf,
            found,
            final_target_disc,
            leafnode,
            use_lp,
            use_sat,
            use_occupancy_cache,
        );
    }
}

fn expand_prev_states(
    node: [u64; 2],
    layer_seen: &DashSet<[u64; 2]>,
    next_layer: &mut Vec<[u64; 2]>,
    retroflips: &mut [u64; 10_000],
    prev_buf: &mut Vec<[u64; 2]>,
    found: &AtomicBool,
    final_target_disc: i32,
    leafnode: &Vec<[u64; 2]>,
    use_lp: bool,
    use_sat: bool,
    use_occupancy_cache: bool,
) {
    prev_states_with_buffer(node, retroflips, prev_buf);
    for prev in prev_buf.iter().copied() {
        if found.load(Ordering::Relaxed) {
            return;
        }

        // 枝刈り
        if !check_edge_patterns(prev[0], prev[1])
            || !check_occupancy(prev[0] | prev[1])
            || !check_seg3_more(prev[0], prev[1], use_occupancy_cache)
            || (use_lp && !check_lp(prev[0], prev[1], false, use_occupancy_cache))
            || (use_sat && !is_sat_ok(prev[0], prev[1], false).unwrap_or(true))
        {
            continue;
        }

        let unique = Board::new(prev[0], prev[1]).unique();

        if !layer_seen.insert(unique) {
            continue; // Already seen in this layer
        }

        // 目標判定
        let disc_count = (unique[0] | unique[1]).count_ones() as i32;
        if disc_count == final_target_disc {
            if is_leaf(unique, leafnode, final_target_disc) {
                found.store(true, Ordering::Release);
                return;
            }
            continue;
        }

        next_layer.push(unique);
    }
}
