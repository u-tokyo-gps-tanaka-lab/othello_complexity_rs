use crate::hash::CustomHash;
use crate::othello::{backshift, Direction, CENTER_MASK};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// 中央4マスから到達可能なoccupied bitboardを計算
///
/// # 前提条件
/// - 中央2x2 (D4, E4, D5, E5) は常に占有されている必要がある
/// - A1 が LSB(bit 0)、H1 が bit 7、A8 が bit 56、H8 が bit 63
//  - 方向は N=+8, S=-8, E=+1, W=-1, NE=+9, NW=+7, SE=-7, SW=-9
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

#[inline(always)]
fn is_center4(sq: usize) -> bool {
    (CENTER_MASK & (1u64 << sq)) != 0
}

static OCCUPANCY_DEPS: OnceLock<([[(u8, u8); 8]; 64], [u8; 64])> = OnceLock::new();

/// 各マスが説明可能になるための局所依存先ペア一覧を前計算する
/// - deps[sq][d] = (a, b) : a=sq+d, b=sq+2dが説明可能ならば、sqも説明可能
/// - dep_len[sq] : 有効なdの総数。すなわち、sqに対応する(a,b)の総数
fn occupancy_deps() -> &'static ([[(u8, u8); 8]; 64], [u8; 64]) {
    OCCUPANCY_DEPS.get_or_init(|| {
        let dirs = Direction::all();
        let mut deps = [[(0u8, 0u8); 8]; 64];
        let mut dep_len = [0u8; 64];

        for sq in 0..64 {
            let x = (sq & 7) as i32;
            let y = (sq >> 3) as i32;
            let mut len = 0usize;
            for dir in dirs.iter() {
                let (dx, dy) = dir.to_offset();
                let x1 = x + dx;
                let y1 = y + dy;
                let x2 = x1 + dx;
                let y2 = y1 + dy;
                if (0..8).contains(&x1)
                    && (0..8).contains(&y1)
                    && (0..8).contains(&x2)
                    && (0..8).contains(&y2)
                {
                    let a = (y1 * 8 + x1) as u8;
                    let b = (y2 * 8 + x2) as u8;
                    deps[sq][len] = (a, b);
                    len += 1;
                }
            }
            dep_len[sq] = len as u8;
        }
        (deps, dep_len)
    })
}

/// 64x64ビット行列（row-major, 1行=1u64）を転置する。
/// - 入力 rows[r] の bit c を、出力 out[c] の bit r に移す
/// - bit 0 は LSB（rows[r] >> 0 が列0）という前提
/// ref: https://lukas-prokop.at/articles/2021-07-23-transpose
#[inline(always)]
fn transpose_bit_matrix_64(mut rows: [u64; 64]) -> [u64; 64] {
    const MASK: [[u64; 2]; 6] = [
        [0x5555_5555_5555_5555u64, 0xAAAA_AAAA_AAAA_AAAAu64],
        [0x3333_3333_3333_3333u64, 0xCCCC_CCCC_CCCC_CCCCu64],
        [0x0F0F_0F0F_0F0F_0F0Fu64, 0xF0F0_F0F0_F0F0_F0F0u64],
        [0x00FF_00FF_00FF_00FFu64, 0xFF00_FF00_FF00_FF00u64],
        [0x0000_FFFF_0000_FFFFu64, 0xFFFF_0000_FFFF_0000u64],
        [0x0000_0000_FFFF_FFFFu64, 0xFFFF_FFFF_0000_0000u64],
    ];

    for j in (0..6).rev() {
        let s: usize = 1usize << j; // 32,16,8,4,2,1
        let sh: u32 = s as u32;

        for p in 0..(32 / s) {
            for i in 0..s {
                let idx0 = p * 2 * s + i;
                let idx1 = idx0 + s;

                let a = rows[idx0];
                let b = rows[idx1];

                let x = (a & MASK[j][0]) | ((b & MASK[j][0]) << sh);
                let y = ((a & MASK[j][1]) >> sh) | (b & MASK[j][1]);

                rows[idx0] = x;
                rows[idx1] = y;
            }
        }
    }
    rows
}

/// 下記の考え方に基づいて、各石の置かれた順序を計算
/// 1. マスAの石を取り除いたら、マスBが説明不可能になった
/// → マスBは、マスAを経由して初めて中心と接続できた
/// → つまり、マスBはマスAの後に置かれた石
/// 2. マスAを取り除いても、マスCが依然として説明可能
/// → マスCは、マスAに依存せずに中心と接続できている
/// → つまり、マスCはマスAと同時またはそれ以前に置かれた石
pub fn occupancy_order(occupied: u64) -> [u64; 64] {
    let (deps, dep_len) = occupancy_deps();

    // alive[w] の bit r:「石 r を除いた局面で石 w が説明可能か」を表す
    // ansとaliveは転置の関係にある
    let mut alive = [0u64; 64];
    for sq in 0..64 {
        if is_center4(sq) {
            // 中央4マスは occupied に含まれなくても常に説明可能
            alive[sq] = u64::MAX;
        }
    }

    // alive[sq] = !(1<<sq) AND (OR over deps (alive[a] AND alive[b])) を計算
    // - 石sqが説明可能であるためには、方向dに対してdeps[dq][d]=(a,b)が説明可能である必要がある
    // - 石sqを除去した場合はbit sqを落とす
    loop {
        let mut changed = false;
        let mut occ = occupied & !CENTER_MASK;
        while occ != 0 {
            let sq = occ.trailing_zeros() as usize;
            occ &= occ - 1;

            let mut support = 0u64;
            for i in 0..dep_len[sq] as usize {
                let (a, b) = deps[sq][i];
                support |= alive[a as usize] & alive[b as usize];
            }

            let next = support & !(1u64 << sq);
            if next != alive[sq] {
                alive[sq] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // ans[r] の bit w:「石 r を除いた局面で石 w が説明可能か」を表す
    let mut ans = [0u64; 64];

    // alive[w] の bit r を ans[r] の bit w に転置する
    // ただしnaive実装と同様に、occupiedに含まれないrについては0を返す。
    let transposed = transpose_bit_matrix_64(alive);
    let mut occ = occupied;
    while occ != 0 {
        let sq = occ.trailing_zeros() as usize;
        occ &= occ - 1;
        ans[sq] = transposed[sq] | (1u64 << sq); // 石sqのbitを立てる
    }
    ans
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

pub fn check_occupancy_with_string(occupied: u64) -> (bool, String) {
    if (occupied & CENTER_MASK) != CENTER_MASK {
        return (false, occupied_to_string(occupied));
    }
    let result = reachable_occupancy(occupied);
    let line = occupied_to_string(result);
    return (result == occupied, line);
}

static OCCUPANCY_ORDER_TT: OnceLock<DashMap<u64, [u64; 64], CustomHash>> = OnceLock::new();
static OCCUPANCY_ORDER_TT_LOOKUPS: AtomicU64 = AtomicU64::new(0);
static OCCUPANCY_ORDER_TT_HITS: AtomicU64 = AtomicU64::new(0);

fn occupancy_order_tt() -> &'static DashMap<u64, [u64; 64], CustomHash> {
    OCCUPANCY_ORDER_TT.get_or_init(|| DashMap::with_hasher(CustomHash::default()))
}

/// Clear the occupancy order transposition table (optional).
pub fn clear_occupancy_cache() {
    if let Some(tt) = OCCUPANCY_ORDER_TT.get() {
        tt.clear();
    }
}

pub fn occupancy_order_cached(occupied: u64) -> [u64; 64] {
    let tt = occupancy_order_tt();
    OCCUPANCY_ORDER_TT_LOOKUPS.fetch_add(1, Ordering::Relaxed);
    if let Some(hit) = tt.get(&occupied) {
        OCCUPANCY_ORDER_TT_HITS.fetch_add(1, Ordering::Relaxed);
        return *hit;
    }

    let result = occupancy_order(occupied);
    tt.insert(occupied, result);
    result
}

#[derive(Clone, Copy, Debug)]
pub struct OccupancyOrderCacheStats {
    pub lookups: u64,
    pub hits: u64,
    pub entries: usize,
}

impl OccupancyOrderCacheStats {
    pub fn misses(&self) -> u64 {
        self.lookups.saturating_sub(self.hits)
    }

    pub fn hit_rate(&self) -> f64 {
        if self.lookups == 0 {
            0.0
        } else {
            (self.hits as f64) / (self.lookups as f64)
        }
    }
}

pub fn occupancy_order_cache_stats() -> OccupancyOrderCacheStats {
    let entries = OCCUPANCY_ORDER_TT.get().map_or(0, |tt| tt.len());
    OccupancyOrderCacheStats {
        lookups: OCCUPANCY_ORDER_TT_LOOKUPS.load(Ordering::Relaxed),
        hits: OCCUPANCY_ORDER_TT_HITS.load(Ordering::Relaxed),
        entries,
    }
}

// ナイーブな実装 (盤上の石全てにreachable_occupancyを呼ぶ)
pub fn occupancy_order_naive(occupied: u64) -> [u64; 64] {
    let mut ans = [0; 64];
    let mut b = occupied;
    while b != 0 {
        let sq = b.trailing_zeros() as usize;
        let newb = b & (b - 1);
        let b_one = b ^ newb; // bからマスsqの石を取り除いた盤面
        ans[sq] = reachable_occupancy(occupied ^ b_one) | b_one; // マスsqと同時またはそれ以前に置かれた石の集合
        b = newb;
    }
    ans
}

#[cfg(test)]
mod tests {
    use super::{occupancy_order, occupancy_order_naive, CENTER_MASK};
    use rand::{Rng, SeedableRng};

    #[test]
    fn occupancy_order_matches_naive_random() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x0ccu64);
        for _ in 0..100000 {
            let occupied = rng.random::<u64>();
            assert_eq!(
                occupancy_order(occupied),
                occupancy_order_naive(occupied),
                "occupied={:#018x}",
                occupied
            );
        }
    }

    #[test]
    fn occupancy_order_matches_naive_random_with_center() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x0cc5eedu64);
        for _ in 0..100000 {
            let occupied = rng.random::<u64>() | CENTER_MASK;
            assert_eq!(
                occupancy_order(occupied),
                occupancy_order_naive(occupied),
                "occupied={:#018x}",
                occupied
            );
        }
    }
}
