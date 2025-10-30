use crate::lib::othello::{east, ne, north, nw, se, south, sw, west, CENTER_MASK};
// 前提：A1 が LSB(bit 0)、H1 が bit 7、A8 が bit 56、H8 が bit 63。
//       方向は N=+8, S=-8, E=+1, W=-1, NE=+9, NW=+7, SE=-7, SW=-9。

// === 方向定義 ===
#[derive(Copy, Clone)]
enum Dir {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

#[inline]
fn backshift(d: Dir, b: u64) -> u64 {
    match d {
        Dir::N => south(b),
        Dir::S => north(b),
        Dir::E => west(b),
        Dir::W => east(b),
        Dir::NE => sw(b),
        Dir::NW => se(b),
        Dir::SE => nw(b),
        Dir::SW => ne(b),
    }
}

pub fn occupied_to_string(o: u64) -> String {
    let mut s = String::new();
    for y in 0..8 {
        for x in 0..8 {
            let i = y * 8 + x;
            if o & (1u64 << i) != 0 {
                s.push('G');
            } else {
                s.push('-');
            }
        }
    }
    return s;
}

pub fn reachable_occupancy(occupied: u64) -> u64 {
    // 必要条件：中央2x2 は常に占有（満たさなければ即不可能）

    let dirs = [
        Dir::N,
        Dir::S,
        Dir::E,
        Dir::W,
        Dir::NE,
        Dir::NW,
        Dir::SE,
        Dir::SW,
    ];

    // 既知に“説明済み”の集合の初期値
    let mut t: u64 = CENTER_MASK;

    // 最悪でも中央4以外の 60 マス分の反復で収束
    for _ in 0..60 {
        let mut add_all: u64 = 0;
        for &d in &dirs {
            // T を O の中で逆方向に押し広げる：
            // W0 = T（距離0）
            // W1 = backshift(d, W0) & O（距離1：不可）
            // W2 = backshift(d, W1) & O（距離2：有効）
            //let w1 = backshift(d, T) & O;
            let w1 = backshift(d, t) & t;
            let mut wk = backshift(d, w1) & occupied; // 距離2スタート（有効）

            let mut r_d = wk; // 方向 d における「距離>=2で見通せる」集合

            // 空きに当たるまで（O に連続する限り）さらに遡る
            while wk != 0 {
                wk = backshift(d, wk) & occupied; // 距離3,4,... と伸ばす
                r_d |= wk;
            }

            add_all |= r_d;
            //println!("i={}", i); i += 1;
        }

        // 未取り込み分だけ追加
        let add = add_all & !t;
        if add == 0 {
            break; // これ以上広がらない → 不動点
        }
        t |= add;

        if t == occupied {
            return t; // 早期収束
        }
    }
    t
}

pub fn check_occupancy(occupied: u64) -> bool {
    if (occupied & CENTER_MASK) != CENTER_MASK {
        return false;
    }
    let result = reachable_occupancy(occupied);
    return result == occupied;
}

pub fn check_occupancy_with_string(occupied: u64) -> (bool, String) {
    if (occupied & CENTER_MASK) != CENTER_MASK {
        return (false, occupied_to_string(occupied));
    }
    let result = reachable_occupancy(occupied);
    let line = occupied_to_string(result);
    return (result == occupied, line);
}

pub fn occupancy_order(occupied: u64) -> [u64; 64] {
    let mut ans = [0; 64];
    let mut b = occupied;
    while b != 0 {
        let sq = b.trailing_zeros() as usize; // 0..=63
        let newb = b & (b - 1);
        let b_one = b ^ newb;
        ans[sq] = reachable_occupancy(occupied ^ b_one) | b_one;
        b = newb;
    }
    ans
}
