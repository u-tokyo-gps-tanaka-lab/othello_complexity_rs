use crossbeam_skiplist::SkipSet;
use dashmap::DashSet;
use ordered_float::NotNan;
use rayon::ThreadPoolBuilder;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::othello::{get_moves, Board, CENTER_MASK};
use crate::prunings::impossible_edges::check_edge_patterns;
use crate::prunings::seg3::check_seg3_more;
use crate::prunings::{linear_programming::check_lp, occupancy::check_occupancy};
use crate::search::core::{prev_states_with_buffer, retrospective_flip, SearchResult};
use crate::search::forward::is_leaf;

// 探索の終了状態
const STATE_RUNNING: u8 = 0;
const STATE_FOUND: u8 = 1;
const STATE_NOT_FOUND: u8 = 2;
const STATE_LIMIT_REACHED: u8 = 3;

thread_local! {
    static RETROFLIPS_BUF: RefCell<[u64; 10_000]> = RefCell::new([0u64; 10_000]);
    static PREV_RETROFLIPS_BUF: RefCell<[u64; 10_000]> = RefCell::new([0u64; 10_000]);
    static PREV_STATES_BUF: RefCell<Vec<[u64; 2]>> = RefCell::new(Vec::new());
}

fn heuristic_function(x: [u64; 2]) -> f64 {
    let board = Board::new(x[0], x[1]);
    RETROFLIPS_BUF.with(|buf| {
        let mut buf = buf.borrow_mut();
        let mut op = board.opponent & !CENTER_MASK;

        // retrospective_flip の総分岐を小さく保つ局面を優先
        // - retrospective_flip は「そのマスが直前手だった」と仮定したときの flip パターン数を返す
        // - 返り値は 1..num の範囲なので、実際の分岐数は (num - 1)
        let mut branching: usize = 0;
        while op != 0 {
            let index = op.trailing_zeros() as usize;
            op &= op - 1;
            let num = retrospective_flip(index, board.player, board.opponent, &mut *buf);
            if num > 1 {
                branching += num - 1;
            }
        }

        // 前局面が生成できない(=分岐0)ものは探索上ほぼ行き止まりなので後回し
        if branching == 0 {
            f64::INFINITY
        } else {
            branching as f64
        }
    })
}

/// 並列 Greedy Best-First Search
/// - use_lp : 線形計画ソルバの枝刈りの有効化
pub fn parallel_retrospective_greedy_best_first_search(
    board: &Board,
    discs: i32,
    leafnode: &Vec<[u64; 2]>,
    node_limit: usize,
    use_lp: bool,
    num_threads: usize,
) -> SearchResult {
    // 共有データ構造
    let open: Arc<SkipSet<(NotNan<f64>, [u64; 2])>> = Arc::new(SkipSet::new());
    let visited: Arc<DashSet<[u64; 2]>> = Arc::new(DashSet::with_capacity(node_limit + 100));
    let visited_count = Arc::new(AtomicUsize::new(0));

    // 終了状態: 0=実行中, 1=発見, 2=未発見, 3=制限到達
    let state = Arc::new(AtomicU8::new(STATE_RUNNING));
    // 現在処理中のノード数（探索枯渇の検出に使用）
    let inflight = Arc::new(AtomicUsize::new(0));

    // 初期ノードを登録
    let mut starts = vec![[board.player, board.opponent]];
    if get_moves(board.opponent, board.player) == 0 {
        starts.push([board.opponent, board.player]);
    }
    for s in starts {
        let unique = Board::new(s[0], s[1]).unique();
        let start = [unique[0], unique[1]];
        if visited.insert(start) {
            visited_count.fetch_add(1, Ordering::Relaxed);
            let h = NotNan::new(heuristic_function(start)).expect("heuristic returned NaN");
            open.insert((h, start));
        }
    }

    // スレッドプール構築
    let num_threads = if num_threads == 0 { 1 } else { num_threads };
    let pool = ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .thread_name(|i| format!("gbfs-worker-{i}"))
        .build()
        .expect("failed to build thread pool");

    // ワーカースレッドを起動
    pool.scope(|scope| {
        for _ in 0..num_threads {
            let open = Arc::clone(&open);
            let visited = Arc::clone(&visited);
            let visited_count = Arc::clone(&visited_count);
            let state = Arc::clone(&state);
            let inflight = Arc::clone(&inflight);

            scope.spawn(move |_| {
                worker_loop(
                    &open,
                    &visited,
                    &visited_count,
                    &state,
                    &inflight,
                    leafnode,
                    discs,
                    node_limit,
                    use_lp,
                );
            });
        }
    });

    // 探索結果を返す
    match state.load(Ordering::Acquire) {
        STATE_FOUND => SearchResult::Found,
        STATE_NOT_FOUND => SearchResult::NotFound,
        _ => SearchResult::Unknown,
    }
}

/// ワーカースレッドのメインループ
fn worker_loop(
    open: &SkipSet<(NotNan<f64>, [u64; 2])>,
    visited: &DashSet<[u64; 2]>,
    visited_count: &AtomicUsize,
    state: &AtomicU8,
    inflight: &AtomicUsize,
    leafnode: &Vec<[u64; 2]>,
    discs: i32,
    node_limit: usize,
    use_lp: bool,
) {
    while state.load(Ordering::Acquire) == STATE_RUNNING {
        // ノード制限チェック
        if visited_count.load(Ordering::Relaxed) >= node_limit {
            let _ = state.compare_exchange(
                STATE_RUNNING,
                STATE_LIMIT_REACHED,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
            break;
        }

        // 優先度キューから最小コストのノードを取得
        let entry = match open.pop_front() {
            Some(e) => e,
            None => {
                // キューが空の場合、探索枯渇を判定
                if is_search_exhausted(open, inflight) {
                    let _ = state.compare_exchange(
                        STATE_RUNNING,
                        STATE_NOT_FOUND,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    );
                }
                continue;
            }
        };

        let node = entry.value().1;
        inflight.fetch_add(1, Ordering::AcqRel);

        // 目標到達判定
        let num_disc = (node[0] | node[1]).count_ones() as i32;
        if num_disc == discs {
            if is_leaf(node, leafnode, discs) {
                let _ = state.compare_exchange(
                    STATE_RUNNING,
                    STATE_FOUND,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                );
                inflight.fetch_sub(1, Ordering::AcqRel);
                break;
            }
            // 目標leaf nodeではなくても、目標と同じ石数に至ったならそれ以上展開しない
            inflight.fetch_sub(1, Ordering::AcqRel);
            continue;
        }

        // LP枝刈り
        if use_lp && !check_lp(node[0], node[1], false, false) {
            inflight.fetch_sub(1, Ordering::AcqRel);
            continue;
        }

        // 子ノードを展開
        expand_node(node, open, visited, visited_count, state, node_limit);

        inflight.fetch_sub(1, Ordering::AcqRel);
    }
}

/// ノードを展開し、子ノードをキューに追加
fn expand_node(
    node: [u64; 2],
    open: &SkipSet<(NotNan<f64>, [u64; 2])>,
    visited: &DashSet<[u64; 2]>,
    visited_count: &AtomicUsize,
    state: &AtomicU8,
    node_limit: usize,
) {
    PREV_RETROFLIPS_BUF.with(|retroflips| {
        PREV_STATES_BUF.with(|prev_buf| {
            let mut retroflips = retroflips.borrow_mut();
            let mut prev_buf = prev_buf.borrow_mut();
            prev_states_with_buffer(node, &mut *retroflips, &mut *prev_buf);

            for prev in prev_buf.iter().copied() {
                if state.load(Ordering::Acquire) != STATE_RUNNING {
                    break;
                }

                // 枝刈りチェック
                let oc = prev[0] | prev[1];
                if !check_occupancy(oc)
                    || !check_seg3_more(prev[0], prev[1], false)
                    || !check_edge_patterns(prev[0], prev[1])
                {
                    continue;
                }

                // 正規化して重複チェック
                let unique = Board::new(prev[0], prev[1]).unique();
                let child = [unique[0], unique[1]];

                if visited.insert(child) {
                    let new_count = visited_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if new_count > node_limit {
                        let _ = state.compare_exchange(
                            STATE_RUNNING,
                            STATE_LIMIT_REACHED,
                            Ordering::AcqRel,
                            Ordering::Relaxed,
                        );
                        break;
                    }
                    if let Ok(h) = NotNan::new(heuristic_function(child)) {
                        open.insert((h, child));
                    }
                }
            }
        });
    });
}

/// 探索が枯渇したか判定（キューが空かつ処理中ノードがない）
fn is_search_exhausted(open: &SkipSet<(NotNan<f64>, [u64; 2])>, inflight: &AtomicUsize) -> bool {
    if inflight.load(Ordering::Acquire) == 0 {
        thread::sleep(Duration::from_micros(50));
        open.is_empty() && inflight.load(Ordering::Acquire) == 0
    } else {
        thread::sleep(Duration::from_micros(50));
        false
    }
}
