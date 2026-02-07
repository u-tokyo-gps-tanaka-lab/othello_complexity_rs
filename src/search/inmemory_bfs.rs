use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use crate::hash::CustomHash;
use dashmap::DashSet;
use rayon::{prelude::*, ThreadPoolBuilder};

use crate::othello::{get_moves, Board};
use crate::prunings::impossible_edges::check_edge_patterns;
use crate::prunings::linear_programming::check_lp;
use crate::prunings::occupancy::check_occupancy;
use crate::prunings::seg3::check_seg3_more;
use crate::search::core::{prev_states_with_buffer, SearchResult};
use crate::search::forward::is_leaf;

/// 探索全体で共有される不変パラメータ
struct SearchContext<'a> {
    closed: &'a DashSet<[u64; 2], CustomHash>,
    found: &'a AtomicBool,
    exceeded: &'a AtomicBool,
    next_count: &'a AtomicUsize,
    current_layer_count: usize,
    node_limit: usize,
    goal_disc_count: i32,
    leafnode: &'a Vec<[u64; 2]>,
    use_lp: bool,
    use_occupancy_cache: bool,
}

impl SearchContext<'_> {
    #[inline]
    fn should_stop(&self) -> bool {
        self.found.load(Ordering::Relaxed) || self.exceeded.load(Ordering::Relaxed)
    }

    /// `closed` への新規登録後に呼び、同時保持ノード上限を超える場合は停止フラグを立てる。
    #[inline]
    fn reserve_slot_after_insert(&self) -> bool {
        if self.should_stop() {
            return false;
        }
        let next = self.next_count.fetch_add(1, Ordering::Relaxed) + 1;
        if self.current_layer_count.saturating_add(next) > self.node_limit {
            self.exceeded.store(true, Ordering::Release);
            return false;
        }
        true
    }

    /// 目標石数に到達していれば true を返す。leafnode に一致する場合は found フラグを立てる。
    fn check_goal(&self, state: [u64; 2]) -> bool {
        let disc_count = (state[0] | state[1]).count_ones() as i32;
        if disc_count == self.goal_disc_count {
            if is_leaf(state, self.leafnode, self.goal_disc_count) {
                self.found.store(true, Ordering::Release);
            }
            return true;
        }
        false
    }
}

/// Parallel in-memory retrospective BFS search
///
/// Searches backward from the goal board state to find if a path exists
/// from the initial Othello position.
///
/// # Parameters
/// - `board`: The start node
/// - `discs`: Target disc count for meeting forward search leaf nodes
/// - `leafnode`: Sorted vector of canonical leaf nodes from forward search
/// - `node_limit`: Maximum number of simultaneously held in-memory nodes
/// - `use_lp`: Enable LP-solver pruning
///
/// # Returns
/// - `SearchResult::Found`: A path exists from initial state to goal
/// - `SearchResult::NotFound`: No path exists (search exhausted or tree unexpandable)
/// - `SearchResult::Unknown`: Resource limit reached before completion
pub fn parallel_inmemory_retrospective_bfs(
    board: &Board,
    discs: i32,
    leafnode: &Vec<[u64; 2]>,
    node_limit: usize,
    use_lp: bool,
    use_occupancy_cache: bool,
) -> SearchResult {
    let initial_disc_count = board.popcount() as i32;
    println!("target_discs={}", initial_disc_count);

    let found: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let exceeded: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    // 早期return
    if initial_disc_count <= discs {
        let unique = board.unique();
        if is_leaf(unique, leafnode, discs) {
            return SearchResult::Found;
        }
        return SearchResult::NotFound;
    }

    // 初期ノードを登録
    let mut current_layer = {
        let u1 = board.unique();
        if get_moves(board.opponent, board.player) == 0 {
            let u2 = Board::new(board.opponent, board.player).unique();
            if u1 == u2 {
                vec![u1]
            } else {
                vec![u1, u2]
            }
        } else {
            vec![u1]
        }
    };
    if current_layer.len() > node_limit {
        return SearchResult::Unknown;
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

    // 石数ごとにlayerを作る
    for i in (discs..initial_disc_count).rev() {
        if found.load(Ordering::Acquire) {
            return SearchResult::Found;
        }
        if exceeded.load(Ordering::Acquire) {
            return SearchResult::Unknown;
        }
        if current_layer.is_empty() {
            return SearchResult::NotFound;
        }

        // 重複排除のチェック用の変数
        // sort + dedupによる重複排除は重いのでチェック専用のDashSetを使う
        let closed: Arc<DashSet<[u64; 2], CustomHash>> =
            Arc::new(DashSet::with_hasher(CustomHash::default()));
        let next_count = AtomicUsize::new(0);

        let ctx = SearchContext {
            closed: &closed,
            found: &found,
            exceeded: &exceeded,
            next_count: &next_count,
            current_layer_count: current_layer.len(),
            node_limit,
            goal_disc_count: discs,
            leafnode,
            use_lp,
            use_occupancy_cache,
        };

        let next_layer = pool.install(|| {
            current_layer
                .par_iter()
                .fold(ThreadLocal::new, |mut tls, &node| {
                    if ctx.should_stop() {
                        return tls;
                    }
                    tls.expand_next_layer(node, &ctx);
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
        if exceeded.load(Ordering::Acquire) {
            return SearchResult::Unknown;
        }

        current_layer = next_layer;
    }

    // 目標判定
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

    /// 入力したノードを処理して1手前を展開する
    fn expand_next_layer(&mut self, node: [u64; 2], ctx: &SearchContext) {
        if ctx.should_stop() {
            return;
        }

        self.generate_predecessors(node, ctx);

        if ctx.should_stop() {
            return;
        }

        // 直前手がパスだった場合は石数が変わらないので同じlayerに加える
        if get_moves(node[1], node[0]) == 0 {
            let pass = Board::new(node[1], node[0]).unique();

            // 重複回避
            if !ctx.closed.insert(pass) {
                return;
            }

            if !ctx.reserve_slot_after_insert() {
                // 上限超過と同時に解発見が起きた場合は Found を優先
                let _ = ctx.check_goal(pass);
                return;
            }

            // 目標判定
            if ctx.check_goal(pass) {
                return;
            }

            self.generate_predecessors(pass, ctx);
        }
    }

    /// 1手前のノードを生成する
    fn generate_predecessors(&mut self, node: [u64; 2], ctx: &SearchContext) {
        prev_states_with_buffer(node, &mut *self.retroflips, &mut self.prev_buf);
        for prev in self.prev_buf.iter().copied() {
            if ctx.should_stop() {
                return;
            }

            // 枝刈り
            if !check_edge_patterns(prev[0], prev[1])
                || !check_occupancy(prev[0] | prev[1])
                || !check_seg3_more(prev[0], prev[1], ctx.use_occupancy_cache)
                || (ctx.use_lp && !check_lp(prev[0], prev[1], false, ctx.use_occupancy_cache))
            {
                continue;
            }

            let unique = Board::new(prev[0], prev[1]).unique();

            // 重複回避
            if !ctx.closed.insert(unique) {
                continue;
            }

            if !ctx.reserve_slot_after_insert() {
                // 上限超過と同時に解発見が起きた場合は Found を優先
                let _ = ctx.check_goal(unique);
                return;
            }

            // 目標判定
            if ctx.check_goal(unique) {
                continue;
            }

            self.next.push(unique);
        }
    }
}
