use crate::othello::{
    backshift, east, ne, north, nw, opposite, se, south, sw, west, Direction, BORDER_MASK,
};

pub const IMPOSSIBLE_PATTERNS: &[&str] =
    &["XOXO", "XOXXOX", "XOXXOOXO", "OXOX", "OXOOXO", "OXOOXXOX"];

/// `eligible` 内の連続するマスだけで禁止パターンが構成されているかを調べる。
fn has_impossible_pattern(player: u64, opponent: u64, eligible: u64, dir: Direction) -> bool {
    let colors = [player & eligible, opponent & eligible];
    IMPOSSIBLE_PATTERNS.iter().any(|pattern| {
        let pattern = pattern.as_bytes();
        let mut ends = colors[usize::from(pattern[0] == b'O')];
        for &ch in &pattern[1..] {
            // ここまで一致した列の末尾を1マス進め、次の色が一致する末尾を残す。
            ends = backshift(opposite(dir), ends) & colors[usize::from(ch == b'O')];
        }
        ends != 0
    })
}

/// 物理的な四辺の禁止パターンを検査する。通過は到達可能性の証明ではない。
/// 仮想辺による追加棄却数の比較と、独立した1次元モデルによる検証に用いる。
pub fn check_edge_patterns(player: u64, opponent: u64) -> bool {
    ![Direction::E, Direction::N]
        .into_iter()
        .any(|dir| has_impossible_pattern(player, opponent, BORDER_MASK, dir))
}

/// 物理辺と仮想辺の禁止パターンを検査する。通過は到達可能性の証明ではない。
///
/// 仮想辺は、各石が線に沿わない3軸のどれからも挟まれない直線部分である。
/// 各軸の対向する隣接2マスの少なくとも一方が、空点または盤外ならよい。
/// 現在の空点は過去にも空点だったため、この条件は全履歴の反転を制限する。
///
/// 禁止パターンを最後に変化させた着手を考える。線外から線に沿って反転するなら
/// 入口側の端の2石が同色になり、線上への着手ならパターン内の異色列を
/// 必ず反転してしまう。初期局面に禁止パターンはないため、初期石を含む
/// 線分にも適用できる。詳しい証明は docs/virtual-edges.md を参照。
pub fn check_virtual_edge_patterns(player: u64, opponent: u64) -> bool {
    let occupied = player | opponent;
    // 各軸について、対向する隣接2マスがともに埋まっているマスを求める。
    // 盤外へ出るビットは方向別のシフトが落とす。
    let axes = [
        (Direction::E, east(occupied) & west(occupied)),
        (Direction::N, north(occupied) & south(occupied)),
        (Direction::NE, ne(occupied) & sw(occupied)),
        (Direction::NW, nw(occupied) & se(occupied)),
    ];
    for (axis, &(direction, _)) in axes.iter().enumerate() {
        let mut transverse = 0;
        for (other, &(_, supported)) in axes.iter().enumerate() {
            if other != axis {
                transverse |= supported;
            }
        }
        if has_impossible_pattern(player, opponent, !transverse, direction) {
            return false;
        }
    }
    true
}
