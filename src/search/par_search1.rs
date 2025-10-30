use crossbeam_skiplist::SkipSet;
use dashmap::DashSet;
use ordered_float::NotNan;
use rayon::ThreadPoolBuilder;
use std::thread;

use crate::othello::{get_moves, Board, CENTER_MASK};
use crate::prunings::check_seg3::check_seg3_more;
use crate::prunings::{check_lp::check_lp, check_occupancy::check_occupancy};
use crate::search::move_ordering::h_function;
use crate::search::search::{retrospective_flip, SearchResult};

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering as Ato},
        Arc,
    },
    time::Duration,
};

/// ===== ユーザー提供の関数 =====
/// これらは別モジュールやFFIで実装済みである前提。
/// ここではシグネチャのみ宣言します。
fn is_leaf(x: [u64; 2], leafnode: &Vec<[u64; 2]>, discs: i32) -> bool {
    let oc = x[0] | x[1];
    if discs == oc.count_ones() as i32 {
        // 実装はユーザー側にて
        if let Ok(_) = leafnode.binary_search(&x) {
            return true;
        }
    }
    false
}

fn heuristic_function(x: [u64; 2]) -> f64 {
    h_function(&Board::new(x[0], x[1]))
}

/// ===== パラメータ =====
const NUM_THREADS: usize = 64; // 64スレッド程度
                               //const NUM_NODES: usize = 1_000_000usize;  // 訪問済みの上限（例）。必要に応じて変更

// retroflips やans のallocateでコストがかかっている．使いまわしをしたほうが節約はできるはず．
fn prev_states(b: [u64; 2]) -> Vec<[u64; 2]> {
    let board = Board::new(b[0], b[1]);
    let mut retroflips = [0u64; 10000];
    let mut op = board.opponent & !CENTER_MASK;
    let mut ans = vec![];
    while op != 0 {
        let index = op.trailing_zeros();
        op &= op - 1;
        let num = retrospective_flip(index, board.player, board.opponent, &mut retroflips);
        for i in 1..num {
            let flipped = retroflips[i];
            let prev = Board {
                player: board.opponent ^ (flipped | (1u64 << index)),
                opponent: board.player ^ flipped,
            };
            ans.push([prev.player, prev.opponent]);
            if get_moves(prev.opponent, prev.player) == 0 {
                ans.push([prev.opponent, prev.player]);
            }
        }
    }
    ans
}

/// 並列 Greedy Best-First Search
/// - start: 初期状態
/// - 戻り値: 見つかった leaf の状態（見つからなければ None）
//--------------------------------------
// 公開エントリ：並列版 retrospective（シグネチャを分けました）
pub fn retrospective_search_parallel1(
    board: &Board,
    discs: i32,
    leafnode: &Vec<[u64; 2]>,
    node_limit: usize,
    _table_limit: usize,
) -> SearchResult {
    // 優先度キュー（ロックフリー SkipSet）
    let pq: Arc<SkipSet<(NotNan<f64>, [u64; 2])>> = Arc::new(SkipSet::new());

    // 訪問済み（HashSet）
    let visited: Arc<DashSet<[u64; 2]>> = Arc::new(DashSet::new());

    // 訪問数
    let visited_count = Arc::new(AtomicUsize::new(0));
    let node_per_stone: Arc<[AtomicUsize; 65]> =
        Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
    let done_per_stone: Arc<[AtomicUsize; 65]> =
        Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));

    // 終了フラグ
    let done = Arc::new(AtomicBool::new(false));
    // ===== 追加: 探索枯渇検出用 =====
    // 現在展開中(取り出して処理中)のノード数
    let inflight = Arc::new(AtomicUsize::new(0));
    // 「未発見で探索が完全に枯渇した」ことを示すフラグ
    let notfound = Arc::new(AtomicBool::new(false));
    // 結果（見つかった leaf）
    let found: Arc<crossbeam::queue::ArrayQueue<[u64; 2]>> =
        Arc::new(crossbeam::queue::ArrayQueue::new(1));
    let mut starts = vec![[board.player, board.opponent]];
    if get_moves(board.opponent, board.player) == 0 {
        starts.push([board.opponent, board.player]);
    }
    // 初期ノードを push（重複を避けるため visited にも登録）
    for s in starts {
        let b = Board::new(s[0], s[1]).unique();
        let start = [b[0], b[1]];
        //let guard = visited.guard();
        //if visited.insert(start, &guard) {
        if visited.insert(start) {
            visited_count.fetch_add(1, Ato::Relaxed);
            let h = NotNan::new(heuristic_function(start)).expect("h_function returned NaN");
            pq.insert((h, start));
        }
    }

    // スレッドプール（最大64）
    let parallelism = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let num_threads = std::cmp::min(NUM_THREADS, parallelism);
    let pool = ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .thread_name(|i| format!("gbfs-worker-{i}"))
        .build()
        .expect("failed to build thread pool");

    // ワーカ（busy-poll による前取り／消費）
    pool.scope(|s| {
        for _tid in 0..num_threads {
            let pq = pq.clone();
            let visited = visited.clone();
            let visited_count = visited_count.clone();
            let done = done.clone();
            let found = found.clone();
            let node_per_stone = node_per_stone.clone();
            let done_per_stone = done_per_stone.clone();
            let inflight = inflight.clone(); // ← 追加
            let notfound = notfound.clone(); // ← 追加
            s.spawn(move |_| {
                // 各スレッドで flurry の epoch guard を保持
                //let guard = visited.guard();

                // メインループ
                while !done.load(Ato::Acquire) {
                    // メモリ上限制御
                    if visited_count.load(Ato::Relaxed) >= node_limit {
                        done.store(true, Ato::Release);
                        break;
                    }

                    // 最小キーを front から取り出し
                    // SkipSet はロックフリー。front() で先頭 Entry にアクセスし、remove() でアトミックに削除。
                    let entry = match pq.front() {
                        Some(e) => e,
                        None => {
                            // ===== 追加: 探索枯渇のロックフリー検出 =====
                            // キューが空→inflight==0 なら他スレッドも処理中でない
                            if inflight.load(Ato::Acquire) == 0 {
                                // ダブルチェック（短い待ちを入れてから再確認すると尚良い）
                                std::thread::sleep(Duration::from_micros(50));
                                if pq.front().is_none() && inflight.load(Ato::Acquire) == 0 {
                                    if done
                                        .compare_exchange(false, true, Ato::AcqRel, Ato::Relaxed)
                                        .is_ok()
                                    {
                                        notfound.store(true, Ato::Release);
                                    }
                                    break;
                                }
                            } else {
                                // どこかが処理中なので少し待つ
                                //eprintln!("tid={}, inflight={}", _tid, inflight.load(Ato::Acquire));
                                std::thread::sleep(Duration::from_micros(50));
                            }
                            continue;
                        }
                    };

                    // remove() は成功時に (Key, Value) を返す
                    let node = entry.value().1;
                    if !entry.remove() {
                        continue;
                    }
                    // ===== 追加: このノードを「処理中」としてカウント =====
                    inflight.fetch_add(1, Ato::AcqRel);
                    // ======
                    let num_disc = (node[0] | node[1]).count_ones() as i32;
                    let _ = &done_per_stone[num_disc as usize].fetch_add(1, Ato::Relaxed);
                    // 目標判定
                    if (node[0] | node[1]).count_ones() as i32 == discs {
                        if is_leaf(node, leafnode, discs) {
                            // 競合で複数見つかるのを避ける：最初の1つだけ採用
                            if done
                                .compare_exchange(false, true, Ato::AcqRel, Ato::Relaxed)
                                .is_ok()
                            {
                                let _ = found.push(node);
                            }

                            break;
                        }
                        // ===== 追加: 処理完了（inflight を減算） =====
                        inflight.fetch_sub(1, Ato::AcqRel);
                        // ============================================
                        continue;
                    }
                    if !check_lp(node[0], node[1], false) {
                        // ===== 追加: 処理完了（inflight を減算） =====
                        inflight.fetch_sub(1, Ato::AcqRel);
                        // ============================================
                        continue;
                    }
                    // 展開
                    let succs = prev_states(node);
                    for s in succs {
                        if done.load(Ato::Acquire) {
                            break;
                        }
                        let occupied = s[0] | s[1];
                        if !check_occupancy(occupied) || !check_seg3_more(s[0], s[1]) {
                            continue;
                        }
                        let succ = Board::new(s[0], s[1]).unique();
                        // 既訪問チェック
                        //let already = visited.contains(&succ, &guard);
                        let already = visited.contains(&succ);
                        if already {
                            continue;
                        }

                        // 先に visited へ CAS 的に登録して重複投入を防ぐ
                        //if visited.insert(succ, &guard) {
                        if visited.insert(succ) {
                            let num_disc = (succ[0] | succ[1]).count_ones();
                            let _ = &node_per_stone[num_disc as usize].fetch_add(1, Ato::Relaxed);
                            let new_count = visited_count.fetch_add(1, Ato::Relaxed) + 1;
                            if new_count > node_limit {
                                done.store(true, Ato::Release);
                                break;
                            }

                            // ヒューリスティック評価
                            // NaN が来たら panic させずにスキップしても良いが、ここでは早期に気付けるようにする
                            let h = match NotNan::new(heuristic_function(succ)) {
                                Ok(hh) => hh,
                                Err(_) => continue, // NaNなら破棄
                            };

                            // 優先度キューへ push
                            pq.insert((h, succ));
                        }
                    }
                    // ===== 追加: このノードの展開が終わったので inflight を減算 =====
                    inflight.fetch_sub(1, Ato::AcqRel);
                    // ============================================================
                }
            });
        }
    });
    for i in 0..=64 {
        eprintln!(
            "{}: {} / {}",
            i,
            done_per_stone[i].load(Ato::Relaxed),
            node_per_stone[i].load(Ato::Relaxed)
        );
    }
    // 結果
    if found.len() > 0 {
        SearchResult::Found
    } else if notfound.load(Ato::Acquire) {
        SearchResult::NotFound
    } else {
        SearchResult::Unknown
    }
}
