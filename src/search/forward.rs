use crate::othello::{flip, get_moves, Board, Color, Direction, Position};
use dashmap::DashSet;
use rayon::ThreadPoolBuilder;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
};

// translated with ChatGPT 4o
/**
 * retrospective-dfs-reversi
 *
 * https://github.com/eukaryo/retrospective-dfs-reversi
 *
 * @date 2020
 * @author Hiroki Takizawa
 */
pub fn fwd_search(
    position: &Position,
    searched: &mut HashSet<Position>,
    leafnode: &mut HashSet<Position>,
    discs: i32,
) {
    let board = &position.board;
    let uni = *position;

    if board.popcount() >= discs as u32 {
        // The boundary includes the state before a pass and terminal states.
        leafnode.insert(uni);
        if get_moves(board.player, board.opponent) != 0 {
            return;
        } else if get_moves(board.opponent, board.player) != 0 {
            let next = Board {
                player: board.opponent,
                opponent: board.player,
            };
            fwd_search(
                &Position {
                    board: next,
                    side_to_move: position.side_to_move.opposite(),
                },
                searched,
                leafnode,
                discs,
            );
        }
        return;
    }

    if !searched.insert(uni) {
        return;
    }

    let mut moves = get_moves(board.player, board.opponent);
    if moves == 0 {
        if get_moves(board.opponent, board.player) != 0 {
            let next = Board {
                player: board.opponent,
                opponent: board.player,
            };
            fwd_search(
                &Position {
                    board: next,
                    side_to_move: position.side_to_move.opposite(),
                },
                searched,
                leafnode,
                discs,
            );
        }
        return;
    }
    // println!("{}", board.show());
    // println!("moves={}", mask_to_moves(moves));
    while moves != 0 {
        let idx = moves.trailing_zeros();
        moves &= moves - 1;

        let flipped = flip(idx as usize, board.player, board.opponent);
        if flipped == 0 {
            continue;
        }
        let next = Board {
            player: board.opponent ^ flipped,
            opponent: board.player ^ (flipped | (1u64 << idx)),
        };
        fwd_search(
            &Position {
                board: next,
                side_to_move: position.side_to_move.opposite(),
            },
            searched,
            leafnode,
            discs,
        );
    }
}

/// 色は考慮せず、occupied bitboardだけを見た確定石
/// occupiedからt_occupiedまで遷移するとき、occupiedのうち裏返らない石を求める
fn get_stable_discs(occupied: u64, t_occupied: u64) -> u64 {
    let mut ans = 0;
    let mut b = occupied;
    while b != 0 {
        let index = b.trailing_zeros();
        b &= b - 1;
        let (x, y) = ((index % 8) as i32, (index / 8) as i32);
        let mut can_flip = false;
        for dir in Direction::all().iter() {
            let (dx, dy) = dir.to_offset();
            let mut x1 = x + dx;
            let mut y1 = y + dy;
            let mut i1 = y1 * 8 + x1;
            while 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && occupied & (1 << i1) != 0 {
                x1 += dx;
                y1 += dy;
                i1 = y1 * 8 + x1;
            }
            if 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && t_occupied & (1 << i1) != 0 {
                let x1 = x - dx;
                let y1 = y - dy;
                let i1 = y1 * 8 + x1;
                if 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && t_occupied & (1 << i1) != 0 {
                    can_flip = true;
                    break;
                }
            }
        }
        if !can_flip {
            ans |= 1 << (y * 8 + x);
        }
    }
    ans
}

/// 向きと絶対色を保ち、目標まで反転できない石が一致するかを検査する。
fn check_fwd(b: &Position, target: &Position) -> bool {
    let o1 = b.board.player | b.board.opponent;
    let o2 = target.board.player | target.board.opponent;
    if o1 & o2 != o1 {
        return false;
    }
    let stable = get_stable_discs(o1, o2);
    let black = |p: &Position| match p.side_to_move {
        Color::Black => p.board.player,
        Color::White => p.board.opponent,
    };
    black(b) & stable == black(target) & stable
}

/// 石discs個の配置を初期配置から全列挙
/// 確定石を使って、目的配置bへのパスが明らかに存在しない盤面を枝刈りする
pub fn make_fwd_table(b: &Position, discs: i32) -> Vec<Position> {
    let discs = discs.min(b.board.popcount() as i32);
    let board = b.board;
    println!("b=\n{}\ndiscs={}", board.show(), discs);
    let target = *b;
    let initial = Position::initial();
    let mut ans = Arc::new(vec![initial]);

    for i in 4..discs {
        let visited: Arc<DashSet<Position>> = Arc::new(DashSet::new());
        let next = Arc::new(AtomicUsize::new(0));
        let mut anslen = ans.len();
        //println!("anslen={}", anslen);

        // スレッドプール構築
        let num_threads = thread::available_parallelism()
            .map(|n| n.get().min(64))
            .unwrap_or(1);
        let pool = ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .thread_name(|i| format!("forward-worker-{i}"))
            .build()
            .expect("failed to build thread pool");

        // ワーカースレッド起動
        pool.scope(|s| {
            for _ in 0..num_threads {
                let visited = visited.clone();
                let ans = ans.clone();
                let next = next.clone();
                anslen = ans.len();
                s.spawn(move |_| {
                    loop {
                        let j = next.fetch_add(1, Ordering::Relaxed);
                        if j >= ans.len() {
                            break; // 仕事がなくなった
                        }
                        let b: Position = ans[j];

                        let mut moves = get_moves(b.board.player, b.board.opponent);
                        //println!("j={}, board={}, moves=0b{:b}", j, Board::new(b.board.player, b.board.opponent).to_string(), moves);
                        while moves != 0 {
                            let idx = moves.trailing_zeros();
                            moves &= moves - 1;
                            let flipped = flip(idx as usize, b.board.player, b.board.opponent);
                            //println!("idx={}, flipped=0b{:b}", idx, flipped);
                            if flipped == 0 {
                                println!(
                                    "flipped==0, idx={}, board=\n{}",
                                    idx,
                                    Board::new(b.board.player, b.board.opponent).show()
                                );
                                continue;
                            }
                            let next = Position {
                                board: Board {
                                    player: b.board.opponent ^ flipped,
                                    opponent: b.board.player ^ (flipped | (1u64 << idx)),
                                },
                                side_to_move: b.side_to_move.opposite(),
                            };
                            if !check_fwd(&next, &target) {
                                //println!("ng = \n{}", Board::new(next.player, next.opponent).show());
                                continue;
                            }
                            let uni = next;
                            //println!("insert visited uni={}", Board::new(uni[0], uni[1]).to_string());
                            //let guard = visited.guard();
                            //visited.insert(uni, &guard);
                            visited.insert(uni);
                            if get_moves(uni.board.player, uni.board.opponent) == 0
                                && get_moves(uni.board.opponent, uni.board.player) != 0
                            {
                                let next1 = Position {
                                    board: uni.board.swapped(),
                                    side_to_move: uni.side_to_move.opposite(),
                                };
                                if !check_fwd(&next1, &target) {
                                    //println!("ng = \n{}", Board::new(next.player, next.opponent).show());
                                    continue;
                                }
                                let uni = next1;
                                //println!("insert visited uni={}", Board::new(uni[0], uni[1]).to_string());
                                //let guard = visited.guard();
                                //visited.insert(uni, &guard);
                                visited.insert(uni);
                            }
                        }
                    }
                });
            }
        });
        // println!("before collect");
        let mut newans = vec![];
        //let guard = visited.guard();
        //for node in visited.iter(&guard) {
        //    newans.push(*node);
        //}
        for node in visited.iter() {
            newans.push(*node);
        }
        // println!("after collect()");
        newans.sort();

        println!("i={}, newans.len() = {}", i + 1, newans.len());
        //for j in 0..newans.len() {
        //    println!("{}", Board::new(newans[j][0], newans[j][1]).to_string());
        //}
        ans = Arc::new(newans);
        // println!("after Arc::new(newans)");
    }
    let mut ans_vec = ans.to_vec();
    ans_vec.sort_unstable();
    ans_vec
}

pub fn is_leaf(x: Position, leafnode: &Vec<Position>, discs: i32) -> bool {
    let oc = x.board.player | x.board.opponent;
    if discs == oc.count_ones() as i32 {
        if let Ok(_) = leafnode.binary_search(&x) {
            return true;
        }
    }
    false
}
