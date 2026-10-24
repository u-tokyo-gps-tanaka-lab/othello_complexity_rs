//! The edge condition must agree with a relaxed one-dimensional Othello game.
//! The oracle uses arrays and the rules of placement and mandatory capture;
//! it does not use production move generation or the forbidden-pattern table.

use std::collections::{HashSet, VecDeque};

use othello_complexity_rs::filters::impossible_edges::{
    check_edge_patterns, check_virtual_edge_patterns,
};

/// Zero denotes an empty square; one and two denote the two disc colours.
fn successors(line: &[u8]) -> Vec<Vec<u8>> {
    let len = line.len() as isize;
    let mut successors = Vec::new();

    // A placement may capture away from this line, so either colour may be
    // placed at any empty square without requiring an on-line capture.
    for (square, &cell) in line.iter().enumerate() {
        if cell != 0 {
            continue;
        }
        for colour in [1, 2] {
            let mut next = line.to_vec();
            next[square] = colour;
            for direction in [-1, 1] {
                let mut cursor = square as isize + direction;
                let mut captured = Vec::new();
                while (0..len).contains(&cursor) && line[cursor as usize] == 3 - colour {
                    captured.push(cursor as usize);
                    cursor += direction;
                }
                if (0..len).contains(&cursor) && line[cursor as usize] == colour {
                    for captured_square in captured {
                        next[captured_square] = colour;
                    }
                }
            }
            successors.push(next);
        }
    }

    // A move outside a segment may capture its monochromatic prefix or
    // suffix. Its anchor is the next opposite-colour disc, or an exterior
    // disc when the entire segment has one colour. An empty square blocks it.
    for (start, direction) in [(0, 1), (len - 1, -1)] {
        let colour = line[start as usize];
        if colour == 0 {
            continue;
        }
        let mut cursor = start;
        let mut captured = Vec::new();
        while (0..len).contains(&cursor) && line[cursor as usize] == colour {
            captured.push(cursor as usize);
            cursor += direction;
        }
        if (0..len).contains(&cursor) && line[cursor as usize] == 0 {
            continue;
        }
        let mut next = line.to_vec();
        for captured_square in captured {
            next[captured_square] = 3 - colour;
        }
        successors.push(next);
    }

    successors
}

fn reachable_lines(len: usize) -> HashSet<Vec<u8>> {
    let empty = vec![0; len];
    let mut seen = HashSet::from([empty.clone()]);
    let mut pending = VecDeque::from([empty]);
    while let Some(line) = pending.pop_front() {
        for next in successors(&line) {
            if seen.insert(next.clone()) {
                pending.push_back(next);
            }
        }
    }
    seen
}

#[test]
fn edge_filters_match_the_independent_line_game_for_all_lengths_and_configurations() {
    for len in 1..=8 {
        let reachable = reachable_lines(len);
        for mut encoding in 0..3_usize.pow(len as u32) {
            let mut line = vec![0; len];
            let mut player = 0;
            let mut opponent = 0;
            for (square, cell) in line.iter_mut().enumerate() {
                *cell = (encoding % 3) as u8;
                encoding /= 3;
                // Embed the segment at A1 on an otherwise empty board.
                // No other direction contains four occupied squares.
                match *cell {
                    1 => player |= 1_u64 << square,
                    2 => opponent |= 1_u64 << square,
                    _ => {}
                }
            }
            let expected = reachable.contains(&line);
            assert_eq!(
                check_edge_patterns(player, opponent),
                expected,
                "physical edge {line:?} disagrees with the independent line game"
            );
            assert_eq!(
                check_virtual_edge_patterns(player, opponent),
                expected,
                "virtual edge {line:?} disagrees with the independent line game"
            );
        }
    }
}
