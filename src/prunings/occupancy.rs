use crate::othello::{backshift, Direction, CENTER_MASK};
// 前提：A1 が LSB(bit 0)、H1 が bit 7、A8 が bit 56、H8 が bit 63。
//       方向は N=+8, S=-8, E=+1, W=-1, NE=+9, NW=+7, SE=-7, SW=-9。

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

/// 中央4マスから到達可能なoccupied bitboardを計算
///
/// # 前提条件
/// - 中央2x2 (D4, E4, D5, E5) は常に占有されている必要がある
///
/// # 戻り値
/// 中央4マスから到達可能なマス目を表すビットマスク
pub fn reachable_occupancy(occupied: u64) -> u64 {
    let dirs = Direction::all();

    // 中央4マスから到達可能であることが確認済みのマスの集合（初期値は中央4マス）
    let mut explained: u64 = CENTER_MASK;

    for _ in 0..60 {
        let mut add_all: u64 = 0;
        for &d in &dirs {
            // 方向dにおいて、既に到達可能な2マスが隣接しているペアを検出
            let w1 = backshift(d, explained) & explained;
            // そのペアからさらに1マス逆方向（合計距離2）にある占有マスを検出開始点とする
            let mut scanning_pos = backshift(d, w1) & occupied;

            // 方向dにおいて、既存の到達可能領域から連続する占有マスで新たに到達可能なマス
            let mut r_d = scanning_pos;

            // 連続する占有マスの鎖を空マス（非占有）に当たるまで逆方向に辿る
            while scanning_pos != 0 {
                scanning_pos = backshift(d, scanning_pos) & occupied; // 距離3,4,... と伸ばす
                r_d |= scanning_pos;
            }

            add_all |= r_d;
        }

        // 今回の反復で新たに到達可能と判明したマス（未追跡分のみ）
        let add = add_all & !explained;
        if add == 0 {
            break; // 新規追加なし → 収束
        }
        explained |= add;

        // 全ての占有マスが到達可能になった場合は早期終了
        if explained == occupied {
            return explained;
        }
    }
    explained
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
