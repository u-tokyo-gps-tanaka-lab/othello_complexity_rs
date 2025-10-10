///
// Rust風：ビットボードで「2ステップ可視の閉包（最小不動点）」テストを行う実装例
// 前提：A1 が LSB(bit 0)、H1 が bit 7、A8 が bit 56、H8 が bit 63。
//       方向は N=+8, S=-8, E=+1, W=-1, NE=+9, NW=+7, SE=-7, SW=-9。

// === 基本マスク ===
const FILE_A: u64 = 0x0101_0101_0101_0101;
const FILE_H: u64 = 0x8080_8080_8080_8080;

// 中央2x2（D4, E4, D5, E5）
//  A1=bit0 とすると、D4=27, E4=28, D5=35, E5=36
const C: u64 = (1u64 << 27) | (1u64 << 28) | (1u64 << 35) | (1u64 << 36);

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

// === シフト（前進方向） ===
// （東西・斜めは“出発側”のファイルで溢れを事前マスクするのが定石）
#[inline]
fn sh_n(b: u64) -> u64 {
    b << 8
}
#[inline]
fn sh_s(b: u64) -> u64 {
    b >> 8
}
#[inline]
fn sh_e(b: u64) -> u64 {
    (b << 1) & !FILE_A
} // H→A への横溢れを除去
#[inline]
fn sh_w(b: u64) -> u64 {
    (b >> 1) & !FILE_H
} // A→H への横溢れを除去
#[inline]
fn sh_ne(b: u64) -> u64 {
    (b & !FILE_H) << 9
} // 出発がH列ならNE不可
#[inline]
fn sh_nw(b: u64) -> u64 {
    (b & !FILE_A) << 7
} // 出発がA列ならNW不可
#[inline]
fn sh_se(b: u64) -> u64 {
    (b & !FILE_H) >> 7
} // 出発がH列ならSE不可
#[inline]
fn sh_sw(b: u64) -> u64 {
    (b & !FILE_A) >> 9
} // 出発がA列ならSW不可

// === 逆向きシフト（T から O を“遡る”ため） ===
#[inline]
fn backshift(d: Dir, b: u64) -> u64 {
    match d {
        Dir::N => sh_s(b),
        Dir::S => sh_n(b),
        Dir::E => sh_w(b),
        Dir::W => sh_e(b),
        Dir::NE => sh_sw(b),
        Dir::NW => sh_se(b),
        Dir::SE => sh_nw(b),
        Dir::SW => sh_ne(b),
    }
}

pub fn show_o(o: u64) {
    eprintln!("");
    for y in 0..8 {
        for x in 0..8 {
            let i = y * 8 + x;
            if o & (1u64 << i) != 0 {
                eprint!("O");
            } else {
                eprint!("-");
            }
        }
        eprintln!("");
    }
}

pub fn reachable_occupancy(occupied: u64) -> u64 {
    //showO(O);
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
    let mut t: u64 = C;

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
            //showO(add_all);
        }

        // 未取り込み分だけ追加
        let add = add_all & !t;
        if add == 0 {
            break; // これ以上広がらない → 不動点
        }
        t |= add;

        //showO(T);
        if t == occupied {
            return t; // 早期収束
        }
    }
    t
}

pub fn check_occupancy(occupied: u64) -> bool {
    if (occupied & C) != C {
        return false;
    }
    reachable_occupancy(occupied) == occupied
}
