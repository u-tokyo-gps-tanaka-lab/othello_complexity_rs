use crate::othello::{flip, get_moves, Board, CENTER_MASK};

use std::{cmp::min, env, path::PathBuf, str::FromStr};

/// Tri-state result for limited search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResult {
    Found,
    NotFound,
    Unknown, // node limit exceeded or resource constraint
}

pub fn default_input_path() -> PathBuf {
    PathBuf::from("board.txt")
}

pub fn default_out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result")
}

pub fn read_env_with_default<T>(key: &str, default: T) -> T
where
    T: FromStr,
{
    env::var(key)
        .ok()
        .and_then(|s| s.parse::<T>().ok())
        .unwrap_or(default)
}

#[allow(dead_code)]
fn mask_to_moves(m: u64) -> String {
    let mut ans: Vec<String> = vec!["[".to_string()];
    for i in 0..64 {
        if m & (1 << i) != 0 {
            let y = i / 8;
            let x = i % 8;
            ans.push(format!("({}, {})", x, y));
        }
    }
    ans.push("]".to_string());
    ans.join(",")
}

#[inline(always)]
pub fn onebit(x: u8) -> bool {
    x & (x - 1) == 0
}

/// pos は opponent が直前に置いた位置 (0..=63)。
/// 「直前の着手が pos だった」と仮定したときに、
/// その着手であり得る “ひっくり返り集合” を result に列挙して個数を返す。
/// 返り値が非ゼロのとき `result[0] == 0`（便宜上）。反復時は 1 から使うこと。
pub fn retrospective_flip(
    pos: usize,
    player: u64,
    opponent: u64,
    result: &mut [u64; 10_000],
) -> usize {
    assert!(pos < 64);
    assert!(((1u64 << pos) & opponent) != 0); // posに相手石がある
    assert!(((1u64 << pos) & 0x0000_0018_1800_0000u64) == 0); // posが中央4マスでない

    // 直前にopponentがマスposに着手したという仮定が成り立つかチェック;
    // posへの着手が本当に直前手ならば、その石だけを取り除いた盤面でopponentはplayerをflipできない
    // flipできる場合、直前手でflipされなかった石があることになり矛盾
    if flip(pos, opponent ^ (1u64 << pos), player) != 0 {
        return 0;
    }

    let xpos = (pos % 8) as i32;
    let ypos = (pos / 8) as i32;

    let mut answer: usize = 0;

    // ユーティリティ：answer==0 のとき初期化、それ以外は直積結合
    #[inline]
    fn add_direction_sets(
        answer: &mut usize,
        result: &mut [u64; 10_000],
        acc_bits_seq: impl Iterator<Item = u64>,
    ) {
        if *answer == 0 {
            // 初回：result[0] = 0、以後は累積ORで 1..n-1 を埋める
            result[0] = 0;
            *answer = 1;
            for bits in acc_bits_seq {
                debug_assert!(*answer < result.len());
                result[*answer] = result[*answer - 1] | bits;
                *answer += 1;
            }
        } else {
            // 2 回目以降：既存 0..old_answer-1 に対して各累積方向 bits を OR した新要素を追加
            let old_answer = *answer;
            let mut direction: u64 = 0;
            for bits in acc_bits_seq {
                direction |= bits;
                for j in 0..old_answer {
                    debug_assert!(*answer < result.len());
                    result[*answer] = result[j] | direction;
                    *answer += 1;
                }
            }
        }
    }

    // 上方向（-8）
    if ypos >= 2 {
        let mut length = 0;
        loop {
            let next = pos as i32 - ((length + 1) * 8);
            if next < 0 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == ypos {
                break;
            }
        }
        if length >= 2 {
            // 1..=length-1 個を候補として累積
            let len = length as usize;
            let seq = (1..len).map(|i| 1u64 << (pos - (i * 8)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 下方向（+8）
    if ypos < 6 {
        let mut length = 0;
        loop {
            let next = pos as i32 + ((length + 1) * 8);
            if next > 63 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == 7 - ypos {
                break;
            }
        }
        if length >= 2 {
            let len = length as usize;
            let seq = (1..len).map(|i| 1u64 << (pos + (i * 8)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 右方向（-1）
    if xpos >= 2 {
        let mut length = 0;
        loop {
            let next = pos as i32 - (length + 1);
            if next < 0 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == xpos {
                break;
            }
        }
        if length >= 2 {
            let len = length as usize;
            let seq = (1..len).map(|i| 1u64 << (pos - i));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 左方向（+1）
    if xpos < 6 {
        let mut length = 0;
        loop {
            let next = pos as i32 + (length + 1);
            if next > 63 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == 7 - xpos {
                break;
            }
        }
        if length >= 2 {
            let len = length as usize;
            let seq = (1..len).map(|i| 1u64 << (pos + i));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 右上（-9）
    if xpos >= 2 && ypos >= 2 {
        let mut length = 0;
        loop {
            let next = pos as i32 - ((length + 1) * 9);
            if next < 0 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == min(xpos, ypos) {
                break;
            }
        }
        if length >= 2 {
            let len = length as usize;
            let seq = (1..len).map(|i| 1u64 << (pos - (i * 9)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 左下（+9）
    if xpos < 6 && ypos < 6 {
        let mut length = 0;
        loop {
            let next = pos as i32 + ((length + 1) * 9);
            if next > 63 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == min(7 - xpos, 7 - ypos) {
                break;
            }
        }
        if length >= 2 {
            let len = length as usize;
            let seq = (1..len).map(|i| 1u64 << (pos + (i * 9)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 左上（-7）
    if xpos < 6 && ypos >= 2 {
        let mut length = 0;
        loop {
            let next = pos as i32 - ((length + 1) * 7);
            if next < 0 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == min(7 - xpos, ypos) {
                break;
            }
        }
        if length >= 2 {
            let len = length as usize;
            let seq = (1..len).map(|i| 1u64 << (pos - (i * 7)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    // 右下（+7）
    if xpos >= 2 && ypos < 6 {
        let mut length = 0;
        loop {
            let next = pos as i32 + ((length + 1) * 7);
            if next > 63 {
                break;
            }
            if ((1u64 << (next as u32)) & opponent) != 0 {
                length += 1;
            } else {
                break;
            }
            if length == min(xpos, 7 - ypos) {
                break;
            }
        }
        if length >= 2 {
            let len = length as usize;
            let seq = (1..len).map(|i| 1u64 << (pos + (i * 7)));
            add_direction_sets(&mut answer, result, seq);
        }
    }

    answer
}

// retroflips やans のallocateでコストがかかっている．使いまわしをしたほうが節約はできるはず．
pub fn prev_states(b: [u64; 2]) -> Vec<[u64; 2]> {
    let board = Board::new(b[0], b[1]);
    let mut retroflips = [0u64; 10000];
    let mut op = board.opponent & !CENTER_MASK;
    let mut ans = vec![];

    while op != 0 {
        let index = op.trailing_zeros() as usize;
        op &= op - 1;
        let num = retrospective_flip(index, board.player, board.opponent, &mut retroflips);
        for i in 1..num {
            let flipped = retroflips[i];
            let prev = Board {
                player: board.opponent ^ (flipped | (1u64 << index)),
                opponent: board.player ^ flipped,
            };
            ans.push([prev.player, prev.opponent]);
            if get_moves(prev.opponent, prev.player) == 0 {
                ans.push([prev.opponent, prev.player]);
            }
        }
    }
    ans
}

// retroflipsを使い回すパターン
pub fn prev_states_with_buffer(b: [u64; 2], retroflips: &mut [u64; 10_000]) -> Vec<[u64; 2]> {
    let board = Board::new(b[0], b[1]);
    let mut op = board.opponent & !CENTER_MASK;
    let mut ans = vec![];

    while op != 0 {
        let index = op.trailing_zeros() as usize;
        op &= op - 1;

        let num = retrospective_flip(index, board.player, board.opponent, retroflips);
        for i in 1..num {
            let flipped = retroflips[i];
            let prev = Board {
                player: board.opponent ^ (flipped | (1u64 << index)),
                opponent: board.player ^ flipped,
            };
            ans.push([prev.player, prev.opponent]);
            if get_moves(prev.opponent, prev.player) == 0 {
                ans.push([prev.opponent, prev.player]);
            }
        }
    }
    ans
}
