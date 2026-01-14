use crate::othello::{
    moore_neighborhood, not_rank_1, not_rank_8, opposite, Board, Direction, BORDER_MASK,
};

pub const IMPOSSIBLE_PATTERNS: &[&str] =
    &["XOXO", "XOXXOX", "XOXXOOXO", "OXOX", "OXOOXO", "OXOOXXOX"];

/// 1マスぶんの石配置が指定文字に合致するかを判定するヘルパー
/// - `p_has`: 自石があるか
/// - `o_has`: 相手石があるか
/// - `ch`: パターン文字（X/O/-/?）
#[inline]
pub fn match_char_pattern(p_has: bool, o_has: bool, ch: u8) -> bool {
    match ch {
        b'X' | b'x' => p_has && !o_has,
        b'O' | b'o' => o_has,
        b'-' | b'_' => !p_has && !o_has,
        b'?' => true,
        _ => false,
    }
}

/// ビットボードから特定の縦の列 (file=0..7) のビットだけを取り出し、8bitに圧縮して返す
/// @cite https://www.chessprogramming.org/Occupancy_of_any_Line
#[inline]
fn gather_file(bb: u64, file: u8) -> u8 {
    debug_assert!(file < 8);
    // 1. (bb >> file) で、左からfile列目の垂直方向のラインを左端に寄せる
    // 2. & 0x0101_0101_0101_0101 で左端のラインだけを取り出す
    // 3. 0x0102_0408_1020_4080 を掛けてから 56bit 右シフトすると、離れていた列ビットが8bit整数に収まる
    let masked = (bb >> file) & 0x0101_0101_0101_0101;
    ((masked.wrapping_mul(0x0102_0408_1020_4080)) >> 56) as u8
}

/// 四辺それぞれの player/opponent を 8bit にしたタプル配列を返す（物理的な四辺のみ）
#[inline]
pub fn edge_bytes(board: &Board) -> [(u8, u8); 4] {
    let p = board.player;
    let o = board.opponent;
    [
        ((p & !not_rank_1()) as u8, (o & !not_rank_1()) as u8), // 上辺（rank1）
        (
            ((p & !not_rank_8()) >> 56) as u8,
            ((o & !not_rank_8()) >> 56) as u8,
        ), // 下辺（rank8）
        (gather_file(p, 0), gather_file(o, 0)),                 // 左辺（fileA）
        (gather_file(p, 7), gather_file(o, 7)),                 // 右辺（fileH）
    ]
}

/// u8 bitboardが、MSBからstart番目を起点としてパターンpatを含むか判定する
#[inline]
fn match_pattern_in_edge(p: u8, o: u8, start: u8, pat: &[u8]) -> bool {
    for (i, ch) in pat.iter().enumerate() {
        let bit = 1u8 << (start + i as u8); // 調べたい位置(start+i)だけが1のマスク
        let p_has = p & bit != 0;
        let o_has = o & bit != 0;
        if !match_char_pattern(p_has, o_has, *ch) {
            return false;
        }
    }
    true
}

/// 物理的な四辺（盤の外周1段）だけを対象に、単一パターン（長さ1..8）をチェックする
fn edge_has_pattern(board: &Board, pattern: &str) -> bool {
    let pat = pattern.as_bytes();
    if pat.is_empty() || pat.len() > 8 {
        return false;
    }

    for (p_edge, o_edge) in edge_bytes(board) {
        for start in 0..=8 - pat.len() {
            if match_pattern_in_edge(p_edge, o_edge, start as u8, pat) {
                return true;
            }
        }
    }
    false
}

/// 物理的な四辺（盤の外周1段）に IMPOSSIBLE_PATTERNS が含まれるかをチェックし、
/// - 含まれるなら `false`
/// - 含まれないなら `true`
pub fn check_edge_patterns(player: u64, opponent: u64) -> bool {
    let board = Board::new(player, opponent);
    !IMPOSSIBLE_PATTERNS
        .iter()
        .any(|p| edge_has_pattern(&board, p))
}

/// 盤の四辺から8近傍を辿って到達できる「外部と連結した空点」の集合を求める
/// 1. 盤の四辺上の空点を初期値とする
/// 2. そこから空点のみを8近傍でflood fillし、外部と繋がる全空点を集める
#[inline]
fn outside_empty_mask(board: &Board) -> u64 {
    let occupied = board.player | board.opponent;
    let empty = !occupied;

    let mut open = empty & BORDER_MASK;
    let mut fringe = open;

    while fringe != 0 {
        let next = moore_neighborhood(fringe) & empty & !open;
        if next == 0 {
            break;
        }
        open |= next;
        fringe = next;
    }

    open
}

/// 盤の四辺に連結する空点とその隣接マスを合わせたビットマスクを返す
#[inline]
fn edge_like_mask(board: &Board) -> u64 {
    let open = outside_empty_mask(board);
    open | moore_neighborhood(open) | BORDER_MASK
}

/// 与えられたマス座標の組 positions にパターン pat が一致するか判定。
/// 'X'/'O' は石の有無を厳密チェック、'-' は空点限定、'?' はワイルドカード。
#[inline]
fn match_pattern_in_board(board: &Board, pat: &[u8], positions: &[usize]) -> bool {
    debug_assert!(pat.len() == positions.len());
    let p = board.player;
    let o = board.opponent;
    for (pos, ch) in positions.iter().zip(pat.iter()) {
        let bit = 1u64 << *pos;
        let p_has = p & bit != 0;
        let o_has = o & bit != 0;
        if !match_char_pattern(p_has, o_has, *ch) {
            return false;
        }
    }
    true
}

#[inline]
fn step_index(idx: usize, dir: Direction) -> Option<usize> {
    // 0..63 の一次元 index を (x,y) に直して1ステップ進め、盤内なら新しい index を返す。
    debug_assert!(idx < 64);
    let (x, y) = (idx as i32 % 8, idx as i32 / 8);
    let (dx, dy) = dir.to_offset();
    let nx = x + dx;
    let ny = y + dy;
    if (0..8).contains(&nx) && (0..8).contains(&ny) {
        Some((ny * 8 + nx) as usize)
    } else {
        None
    }
}

/// 盤の実エッジに加え、外部と隣接するマスを「辺」と見なしてパターン（長さ1..8）を検出する
fn board_has_pattern(board: &Board, pattern: &str) -> bool {
    let pat = pattern.as_bytes();
    if pat.is_empty() || pat.len() > 8 {
        return false;
    }

    // 辺相当のマスだけを対象とする
    let edge_mask = edge_like_mask(board);

    // 同一ラインを重複走査しないため、4方向のみで扱う
    for dir in [Direction::E, Direction::S, Direction::SE, Direction::NE] {
        for idx in 0..64 {
            let bit = 1u64 << idx;
            if edge_mask & bit == 0 {
                continue;
            }

            // ラインの先頭でなければスキップ
            if let Some(prev) = step_index(idx, opposite(dir)) {
                if edge_mask & (1u64 << prev) != 0 {
                    continue;
                }
            }

            // 辺相当マスだけで構成される連続ラインを構築
            let mut line: Vec<usize> = Vec::with_capacity(8);
            let mut cur = idx;
            loop {
                if edge_mask & (1u64 << cur) == 0 {
                    break;
                }
                line.push(cur);
                if let Some(next) = step_index(cur, dir) {
                    cur = next;
                } else {
                    break;
                }
            }

            if line.len() < pat.len() {
                continue;
            }

            for start in 0..=line.len() - pat.len() {
                let window = &line[start..start + pat.len()];
                if match_pattern_in_board(board, pat, window) {
                    return true;
                }
            }
        }
    }

    false
}

/// 盤の実エッジに加え仮想エッジ（外部と連結した空点＋隣接）も「辺」と見なして
/// IMPOSSIBLE_PATTERNS が含まれるかをチェックし、
/// - 含まれるなら `false`
/// - 含まれないなら `true`
pub fn check_virtual_edge_patterns(player: u64, opponent: u64) -> bool {
    let board = Board::new(player, opponent);
    !IMPOSSIBLE_PATTERNS
        .iter()
        .any(|p| board_has_pattern(&board, p))
}

#[cfg(test)]
mod tests {
    use super::{board_has_pattern, edge_has_pattern, Board};
    use crate::io::parse_line_to_board;

    fn board_from_str(s: &str) -> Board {
        parse_line_to_board(s).expect("invalid board string")
    }

    #[test]
    fn detects_horizontal_virtual_edge() {
        let b = board_from_str(concat!(
            "--------", // 1
            "---XOXO-", // 2
            "---XOXOX", // 3
            "--XXOXOX", // 4
            "-OOOOOOX", // 5
            "--XXOOX-", // 6
            "-XXXXO--", // 7
            "--------", // 8
        ));

        assert!(board_has_pattern(&b, "XOXO")); // D2-G2 に XOXO
        assert!(board_has_pattern(&b, "XXX"));
        assert!(board_has_pattern(&b, "OXX"));
        assert!(board_has_pattern(&b, "OOO") == false);
        assert!(edge_has_pattern(&b, "XXX"));
        assert!(edge_has_pattern(&b, "OOO") == false);
    }

    #[test]
    fn detects_diagonal_virtual_edge() {
        let b = board_from_str(concat!(
            "--------", // 1
            "--------", // 2
            "-OOOO---", // 3
            "XXOOXX--", // 4
            "XXXOXX--", // 5
            "-OXXXXXX", // 6
            "--XXOO--", // 7
            "---OOO--", // 8
        ));

        assert!(board_has_pattern(&b, "XOXO")); // A5-D8 の斜め XOXO
        assert!(board_has_pattern(&b, "OOO"));
        assert!(edge_has_pattern(&b, "OOO"));
        assert!(board_has_pattern(&b, "XX"));
    }

    #[test]
    fn detects_vertical_virtual_edge() {
        let b = board_from_str(concat!(
            "---XXOOO", // 1
            "---XOXOO", // 2
            "---OXXOO", // 3
            "---XXXOO", // 4
            "---XOOOO", // 5
            "---OXOOO", // 6
            "---XXXOO", // 7
            "---XOXOO", // 8
        ));

        assert!(board_has_pattern(&b, "XOXXOX")); // D2-D7 に XOXXOX
        assert!(edge_has_pattern(&b, "XOXXOX") == false);

        assert!(board_has_pattern(&b, "XXOOO"));
        assert!(edge_has_pattern(&b, "XXOOO"));

        assert!(board_has_pattern(&b, "XOXOO"));
        assert!(edge_has_pattern(&b, "XOXOO"));

        assert!(board_has_pattern(&b, "OOOOO"));
        assert!(edge_has_pattern(&b, "OOOOO"));

        assert!(board_has_pattern(&b, "XXXXX") == false);
        assert!(edge_has_pattern(&b, "XXXXX") == false);
    }

    #[test]
    fn show_boards() {}
}
