
use rayon::ThreadPoolBuilder;
use crate::lib::othello::{flip, get_moves, Board, DXYS};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use dashmap::DashSet;
const NUM_THREADS: usize = 64;            // 64スレッド程度

fn get_stable_discs(occupied: u64, t_occupied: u64) -> u64 {
    let mut ans = 0;
    let mut b = occupied;
    while b != 0 {
        let index = b.trailing_zeros();
        b &= b - 1;
        let (x, y) = ((index % 8) as i32, (index / 8) as i32);
        let mut can_flip = false;
        for (dx, dy) in DXYS.iter(){
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

fn check_fwd_sub(b: &[u64;2], target: &[u64;2]) -> bool {
    let o1 = b[0] | b[1];
    let o2 = target[0] | target[1];
    if o1 & o2 != o1 {
        return false;
    }
    let stable = get_stable_discs(o1, o2);
    if b[0] & stable == target[0] & stable && b[1] & stable == target[1] & stable {
        return true;
    }
    b[0] & stable == target[1] & stable && b[1] & stable == target[0] & stable
}

fn check_fwd(b: &[u64;2], target: &[[u64;2];8]) -> bool {
    for i in 0..8 {
        if check_fwd_sub(b, &target[i]) {
            return true;
        }
    }
    false
}

pub fn make_fwd_table(b: &[u64;2], discs: i32) -> Vec<[u64;2]> {
    let board = Board::new(b[0], b[1]);
    println!("b=\n{}\n, discs={}", board.show(), discs);
    let mut target = [*b;8];
    for i in 1..8 {
        board.board_symmetry(i, &mut target[i as usize]);
    }
    let initial = Board::initial();
    let mut ans = Arc::new(vec![[initial.player, initial.opponent]]);
    for i in 4..discs {
        let visited: Arc<DashSet<[u64; 2]>> = Arc::new(DashSet::new());
        let next = Arc::new(AtomicUsize::new(0));
        let mut anslen = ans.len();
        //println!("anslen={}", anslen);
        let pool = ThreadPoolBuilder::new()
            .num_threads(NUM_THREADS)
            .thread_name(|i| format!("gbfs-worker-{i}"))
            .build()
            .expect("failed to build thread pool");
        pool.scope(|s| {
            for _tid in 0..NUM_THREADS {
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
                        let b: [u64;2] = ans[j];
                        
                        let mut moves = get_moves(b[0], b[1]);
                        //println!("j={}, board={}, moves=0b{:b}", j, Board::new(b[0], b[1]).to_string(), moves);
                        while moves != 0 {
                            let idx = moves.trailing_zeros();
                            moves &= moves - 1;
                            let flipped = flip(idx as usize, b[0], b[1]);
                            //println!("idx={}, flipped=0b{:b}", idx, flipped);
                            if flipped == 0 {
                                println!("flipped==0, idx={}, board=\n{}", idx, Board::new(b[0], b[1]).show());
                                continue;
                            }
                            let next = Board {
                                player: b[1] ^ flipped,
                                opponent: b[0] ^ (flipped | (1u64 << idx)),
                            };
                            if !check_fwd(&[next.player, next.opponent], &target) {
                                //println!("ng = \n{}", Board::new(next.player, next.opponent).show());
                                continue;
                            }
                            let uni = next.unique();
                            //println!("insert visited uni={}", Board::new(uni[0], uni[1]).to_string());
                            //let guard = visited.guard();
                            //visited.insert(uni, &guard);
                            visited.insert(uni);
                            if get_moves(uni[0], uni[1]) == 0 {
                                let next1 = Board {player: uni[1], opponent: uni[0]};
                                if !check_fwd(&[next1.player, next1.opponent], &target) {
                                    //println!("ng = \n{}", Board::new(next.player, next.opponent).show());
                                    continue;
                                }
                                let uni = next1.unique();
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
        println!("before collect");
        let mut newans = vec![];
        //let guard = visited.guard();
        //for node in visited.iter(&guard) {
        //    newans.push(*node);
        //}
        for node in visited.iter() {
            newans.push(*node);
        }
        println!("after collect()");
        newans.sort();
        
        println!("i={}, newans.len() = {}", i, newans.len());
        //for j in 0..newans.len() {
        //    println!("{}", Board::new(newans[j][0], newans[j][1]).to_string());
        //}
        ans = Arc::new(newans);
        println!("after Arc::new(newans)");
    }
    ans.to_vec()
}