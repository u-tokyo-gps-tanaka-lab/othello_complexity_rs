use crate::{
    othello::{Board, DXYS},
    prunings::check_occupancy::occupancy_order,
};

pub fn no_cycle(g: Vec<Vec<usize>>) -> bool {
    let mut icount = vec![0; 64];
    for i in 0..64 {
        for &j in &g[i] {
            icount[j] += 1;
        }
    }
    let mut q = vec![];
    for i in 0..64 {
        if g[i].len() > 0 && icount[i] == 0 {
            q.push(i);
        }
    }
    while q.len() > 0 {
        let i = q.pop().unwrap();
        for &j in &g[i as usize] {
            icount[j] -= 1;
            if icount[j] == 0 {
                q.push(j);
            }
        }
    }
    for i in 0..64 {
        if icount[i] > 0 {
            return false;
        }
    }
    true
}

/// 下記の2条件によって到達不能な局面を検出する
/// 矛盾が生じる（到達不能）ならfalse, 無矛盾ならtrue
///
/// cond 1:
/// ある石から見て、8方向のどの方向にも連続した3つ以上の石が存在しない場合、その石を置いた際にflip操作が起こらなかったことになり、矛盾する。
/// 従って局面が初期配置から到達可能であるためには、全ての石について8方向のうち必ず1つ以上の方向で3つ以上の石が連続している必要がある
///
/// cond 2:
/// 局面 $s$ について、64個の各マスを頂点とし、マス$i$への着手がマス$j$に依存している際に $i$ から $j$ への有向辺を持つ有向グラフ $G_s$ を作成する（ただし $i \neq j$）。
/// $G_s$ に閉路が存在するならば、$G_s$に対応する局面$s$は初期局面から到達不能である。
/// 閉路が存在することは「着手の依存関係に循環がある」ことを意味し、矛盾する。
pub fn check_seg3(b: u64) -> bool {
    let mut g: Vec<Vec<usize>> = vec![vec![]; 64];
    for y in 0..8 {
        for x in 0..8 {
            let i = y * 8 + x;
            if b & (1 << i) == 0 {
                continue;
            }
            if 3 <= x && x <= 4 && 3 <= y && y <= 4 {
                continue;
            }
            let mut oks: Vec<Vec<usize>> = vec![];
            for (dx, dy) in DXYS.iter() {
                let mut l = 1;
                let mut x1 = x + dx;
                let mut y1 = y + dy;
                let mut i1 = y1 * 8 + x1;
                while 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && b & (1 << i1) != 0 {
                    l += 1;
                    x1 += dx;
                    y1 += dy;
                    i1 = y1 * 8 + x1;
                }
                if l >= 3 {
                    let di = dy * 8 + dx;
                    oks.push(vec![(i + di) as usize, (i + di * 2) as usize]);
                }
            }
            if oks.len() == 0 {
                return false;
            }
            if oks.len() == 1 {
                g[i as usize].push(oks[0][0]);
                g[i as usize].push(oks[0][1]);
            }
        }
    }
    return no_cycle(g);
}

fn can_put_flip(occupied: u64, order: &[u64; 64]) -> ([u8; 64], [u8; 64]) {
    let mut canput: [u8; 64] = [0; 64];
    let mut canflip: [u8; 64] = [0; 64];
    for y in 0..8 {
        for x in 0..8 {
            let i = y * 8 + x;
            if occupied & (1 << i) == 0 {
                continue;
            }
            let mut ls: [u8; 8] = [0; 8];
            let mut ls1: [u8; 8] = [0; 8];
            let o1 = order[i as usize];
            for (d, (dx, dy)) in DXYS.iter().enumerate() {
                let mut l = 1;
                let mut l1 = 1;
                let mut x1 = x + dx;
                let mut y1 = y + dy;
                let mut i1 = y1 * 8 + x1;
                let mut in_o1 = true;
                while 0 <= x1 && x1 < 8 && 0 <= y1 && y1 < 8 && occupied & (1 << i1) != 0 {
                    if in_o1 && o1 & (1 << i1) == 0 {
                        in_o1 = false;
                    }
                    if in_o1 {
                        l1 += 1;
                    }
                    l += 1;
                    x1 += dx;
                    y1 += dy;
                    i1 = y1 * 8 + x1;
                }
                ls[d] = l;
                ls1[d] = l1;
            }
            for d in 0..8 {
                if ls1[d] >= 3 {
                    if !(3 <= x && x <= 4 && 3 <= y && y <= 4) {
                        canput[i as usize] |= 1u8 << d;
                    }
                }
                if d < 4 && ls[d] >= 2 && ls[d + 4] >= 2 {
                    canflip[i as usize] |= 1u8 << d;
                }
            }
        }
    }
    (canput, canflip)
}

/// 盤面が初期配置に到達不能かどうかの粗めのチェック．
pub fn check_seg3_more(player: u64, opponent: u64) -> bool {
    //if !check_seg3_more(player, opponent) {
    //    return false;
    //}

    let occupied = player | opponent;
    let order = occupancy_order(occupied);
    let (canput, canflip) = can_put_flip(occupied, &order);
    let ps = [player, opponent];
    for i in 0..2 {
        let p0 = ps[i];
        for y in 0..8 {
            for x in 0..8 {
                if 3 <= x && x <= 4 && 3 <= y && y <= 4 {
                    continue;
                }
                let i = y * 8 + x;
                if p0 & (1 << i) == 0 {
                    continue;
                }
                if canput[i as usize] == 0 {
                    eprintln!("canput = 0, i={}, x={}, y={}", i, x, y);
                    eprintln!("{}", Board::new(player, opponent).show());
                    panic!("inconsistent");
                }
                // putの方向が1方向で後でflipされた可能性がない．
                if canflip[i as usize] != 0 {
                    continue;
                }
                let mut mask = canput[i as usize];
                let mut mask_count = 0;
                let mut ng_count = 0;
                while mask != 0 {
                    let d = mask.trailing_zeros();
                    mask_count += 1;
                    mask &= mask - 1;
                    let d1 = d & 3;
                    let (dx, dy) = DXYS[d as usize];
                    let di = dy * 8 + dx;
                    // 隣と，その隣のマスがd1方向以外にflipされる可能性がない．
                    for i1 in [i + di, i + di * 2] {
                        if p0 & (1 << i1) == 0 {
                            let f = canflip[i1 as usize];
                            if f == 0 {
                                ng_count += 1;
                                break;
                            }
                            if f & !(1 << d1) == 0 {
                                if i1 == i + di {
                                    ng_count += 1;
                                    break;
                                }
                                let i2 = i + di;
                                if p0 & (1 << i2) != 0 && canflip[i2 as usize] & !(1 << d1) == 0 {
                                    //eprintln!("x, y, dx, dy, i1 = {:?}", (x, y, dx, dy,i1));
                                    //eprintln!("{}", Board::new(player, opponent).show());
                                    ng_count += 1;
                                    break;
                                }
                            }
                        }
                    }
                }
                if mask_count == ng_count {
                    //eprintln!("x, y = {:?}", (x, y));
                    //eprintln!("{}", Board::new(player, opponent).show());
                    return false;
                }
            }
        }
    }
    true
}
