use dashmap::DashSet;
use rayon::ThreadPoolBuilder;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::othello::{get_moves, Board};
use crate::prunings::occupancy::check_occupancy;
use crate::prunings::seg3::check_seg3_more;
use crate::search::core::{retrospective_flip, SearchResult};
use crate::search::move_ordering::h_function;

// 並列パラメータ（必要なら調整）
const PAR_MAX_DEPTH: usize = 12; // この深さまでは spawn を許可
const PAR_MIN_CHILDREN: usize = 4; // 子の数がこの数以上なら分割を検討

#[inline]
fn should_split(depth: usize, children: usize) -> bool {
    depth < PAR_MAX_DEPTH && children >= PAR_MIN_CHILDREN
}

// thread-local retroflips バッファ
thread_local! {
    static TL_RETRO: RefCell<Vec<[u64; 10_000]>> = RefCell::new(Vec::new());
}

// 並列探索用の共有状態
struct ParShared<'a> {
    leafnode: &'a std::collections::HashSet<[u64; 2]>, // 読み取り専用
    visited: &'a DashSet<[u64; 2]>,                    // 既訪問ユニーク局面
    discs: i32,
    node_limit: usize,
    table_limit: usize,
    node_count: &'a AtomicUsize, // 走査ノード数
    node_per_stone: &'a [AtomicUsize; 65],
    done_per_stone: &'a [AtomicUsize; 65],
    table_count: &'a AtomicUsize, // 走査ノード数

    // 早期停止フラグ: 0=進行中, 1=Found, 2=Unknown(上限超過)
    stop: &'a AtomicUsize,
}

// ユーティリティ：スレッドプール初期化（必要なら呼ぶ）
pub fn init_rayon(num_threads: Option<usize>) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let mut b = ThreadPoolBuilder::new();
        if let Some(n) = num_threads {
            b = b.num_threads(n);
        }
        b.build_global().expect("failed to init global rayon pool");
    });
}

// 公開エントリ：並列版 retrospective（シグネチャを分けました）
pub fn retrospective_search_parallel(
    board: &Board,
    from_pass: bool,
    discs: i32,
    leafnode: &std::collections::HashSet<[u64; 2]>,
    node_limit: usize,
    table_limit: usize,
) -> SearchResult {
    let visited = DashSet::new();
    let node_count = AtomicUsize::new(0);
    let table_count = AtomicUsize::new(0);
    let node_per_stone: [AtomicUsize; 65] = std::array::from_fn(|_| AtomicUsize::new(0));
    let done_per_stone: [AtomicUsize; 65] = std::array::from_fn(|_| AtomicUsize::new(0));
    let stop = AtomicUsize::new(0);

    let shared = ParShared {
        leafnode,
        visited: &visited,
        discs,
        node_limit,
        table_limit,
        node_count: &node_count,
        table_count: &table_count,
        node_per_stone: &node_per_stone,
        done_per_stone: &done_per_stone,
        stop: &stop,
    };

    // ルート呼び出し
    let res = par_retro_core(board, from_pass, &shared, 0);
    for i in 0..=64 {
        eprintln!(
            "{}: {} / {}",
            i,
            done_per_stone[i].load(Ordering::Relaxed),
            node_per_stone[i].load(Ordering::Relaxed)
        );
    }
    res
}

// 動的並列コア
fn par_retro_core(board: &Board, from_pass: bool, sh: &ParShared, depth: usize) -> SearchResult {
    // 全体の早期停止を確認
    match sh.stop.load(Ordering::Relaxed) {
        1 => return SearchResult::Found,
        2 => return SearchResult::Unknown,
        _ => {}
    }

    let uni = board.unique();
    let num_disc = board.popcount() as usize;

    // カウンターの変更
    sh.done_per_stone[num_disc].fetch_add(1, Ordering::Relaxed);

    // しきい以下なら leafnode 照合のみ
    if (num_disc as i32) <= sh.discs {
        let r = if sh.leafnode.contains(&uni) {
            SearchResult::Found
        } else {
            SearchResult::NotFound
        };
        if r == SearchResult::Found {
            let _ = sh
                .stop
                .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed);
        }
        return r;
    }

    // 再訪防止
    let n = sh.table_count.fetch_add(1, Ordering::Relaxed) + 1;
    if n < sh.table_limit {
        if !sh.visited.insert(uni) {
            return SearchResult::NotFound;
        }
    } else {
        if sh.visited.contains(&uni) {
            return SearchResult::NotFound;
        }
    }

    // ノード数制限
    let n = sh.node_count.fetch_add(1, Ordering::Relaxed) + 1;
    if n > sh.node_limit {
        // Unknown（上限超過）を全体に通知
        let _ = sh
            .stop
            .compare_exchange(0, 2, Ordering::Relaxed, Ordering::Relaxed);
        return SearchResult::Unknown;
    }

    // 形状フィルタ
    let occupied = board.player | board.opponent;
    if !check_occupancy(occupied) || !check_seg3_more(board.player, board.opponent) {
        // !is_connected(occupied) || !check_seg3(occupied)
        return SearchResult::NotFound;
    }

    // ---- 子ノード列挙（パス + 直前着手候補からの retroflips）----
    // 1) パス枝（from_pass==false かつ 相手に合法手無し）
    let mut children: Vec<(Board, bool)> = Vec::new(); // (prev_board, from_pass_prev)
    if !from_pass && get_moves(board.opponent, board.player) == 0 {
        children.push((
            Board {
                player: board.opponent,
                opponent: board.player,
            },
            true,
        ));
    }

    // 2) 直前着手位置ごとの “可能 flip 集合” 展開
    let b = board.opponent & !0x0000_0018_1800_0000u64;
    if b == 0 && children.is_empty() {
        return SearchResult::NotFound;
    }

    TL_RETRO.with(|tl| {
        let mut retro = tl.borrow_mut();
        if retro.len() <= num_disc {
            retro.resize(num_disc + 1, [0u64; 10_000]);
        }

        let mut bb = b;
        while bb != 0 {
            let index = bb.trailing_zeros();
            bb &= bb - 1;

            let num = retrospective_flip(index, board.player, board.opponent, &mut retro[num_disc]);
            for i in 1..num {
                let flipped = retro[num_disc][i];
                debug_assert!(flipped != 0);
                let prev = Board {
                    player: board.opponent ^ (flipped | (1u64 << index)),
                    opponent: board.player ^ flipped,
                };
                children.push((prev, false));
            }
        }
    });

    if children.is_empty() {
        return SearchResult::NotFound;
    }
    let csize = children.len();
    // children をh_functrionに従ってソートする．
    let mut children_score: Vec<(f64, usize)> = vec![];
    for i in 0..csize {
        children_score.push((h_function(&children[i].0), i));
    }
    children_score.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut new_children: Vec<(Board, bool)> = vec![];
    for i in 0..csize {
        let j = children_score[i].1;
        new_children.push(children[j]);
    }
    children = new_children;

    sh.node_per_stone[num_disc - 1].fetch_add(csize, Ordering::Relaxed);
    // ---- 動的に並列 or 直列を選ぶ ----
    if should_split(depth, children.len()) {
        use std::sync::atomic::AtomicUsize;
        let local_best = AtomicUsize::new(SearchResult::NotFound as usize);

        rayon::scope_fifo(|s| {
            // children を消費して所有権を取り出す
            let mut it = children.into_iter();

            // 先頭はこのスレッドで実行
            if let Some((bd0, fp0)) = it.next() {
                let r0 = par_retro_core(&bd0, fp0, sh, depth + 1);
                match r0 {
                    SearchResult::Found => {
                        local_best.store(SearchResult::Found as usize, Ordering::Relaxed);
                        let _ =
                            sh.stop
                                .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed);
                    }
                    SearchResult::Unknown => {
                        if local_best.load(Ordering::Relaxed) == (SearchResult::NotFound as usize) {
                            local_best.store(SearchResult::Unknown as usize, Ordering::Relaxed);
                        }
                    }
                    SearchResult::NotFound => {}
                }
            }

            // 残りはタスクとして spawn（move で所有権を渡す）
            // 共有する参照は、参照値を変数に束ねて、それを move でキャプチャ
            let lb_ref = &local_best;
            let sh_ref = sh;

            for (bd, fp) in it {
                s.spawn_fifo(move |_| {
                    // bd と fp は move 済み（所有）
                    let r = par_retro_core(&bd, fp, sh_ref, depth + 1);

                    match r {
                        SearchResult::Found => {
                            lb_ref.store(SearchResult::Found as usize, Ordering::Relaxed);
                            let _ = sh_ref.stop.compare_exchange(
                                0,
                                1,
                                Ordering::Relaxed,
                                Ordering::Relaxed,
                            );
                        }
                        SearchResult::Unknown => {
                            if lb_ref.load(Ordering::Relaxed) == (SearchResult::NotFound as usize) {
                                lb_ref.store(SearchResult::Unknown as usize, Ordering::Relaxed);
                            }
                        }
                        SearchResult::NotFound => {}
                    }
                });
            }
        });

        match local_best.load(Ordering::Relaxed) {
            x if x == (SearchResult::Found as usize) => SearchResult::Found,
            x if x == (SearchResult::Unknown as usize) => SearchResult::Unknown,
            _ => SearchResult::NotFound,
        }
    } else {
        // 直列分岐はそのまま
        for (bd, fp) in children {
            let r = par_retro_core(&bd, fp, sh, depth + 1);
            match r {
                SearchResult::Found => return SearchResult::Found,
                SearchResult::Unknown => return SearchResult::Unknown,
                SearchResult::NotFound => {}
            }
            match sh.stop.load(Ordering::Relaxed) {
                1 => return SearchResult::Found,
                2 => return SearchResult::Unknown,
                _ => {}
            }
        }
        SearchResult::NotFound
    }
}
