//! Training examples for a reverse-search ordering model.
//!
//! An observed predecessor is a demonstrated legal history, not the only legal
//! history. Other candidates are comparison examples, never unreachability
//! labels. This module is not used to decide a reachability query.

use std::collections::HashSet;
use std::io;

use rand::Rng;

use crate::filters::impossible_edges::check_virtual_edge_patterns;
use crate::othello::{flip, Board, Position, CENTER_MASK};
use crate::record::{replay, PlyAction};
use crate::search::constraints::ReverseConstraints;
use crate::search::core::pass_predecessor;
use crate::search::forward::ForwardTable;

/// Uniformly sampled locally legal placement predecessors are supplemented by
/// their legal preceding pass, just as in beam expansion. The distribution over
/// these two-action candidates is consequently not claimed to be uniform.
pub struct TrainingGroup {
    pub candidates: Vec<(Position, bool)>,
    pub placement_combinations: usize,
    pub sampled_placements: usize,
    pub examined_candidates: usize,
    pub filtered_candidates: usize,
    pub draw_cap_reached: bool,
}

struct SquareChoices {
    square: usize,
    prefixes: [[u64; 7]; 8],
    radices: [usize; 8],
    count: usize,
}

impl SquareChoices {
    fn from_position(position: Position, mut squares: u64) -> Vec<Self> {
        let Board { player, opponent } = position.board;
        squares &= opponent & !CENTER_MASK;
        let mut blocks = Vec::new();
        while squares != 0 {
            let square = squares.trailing_zeros() as usize;
            squares &= squares - 1;
            if flip(square, opponent ^ (1_u64 << square), player) != 0 {
                continue;
            }
            let mut block = Self {
                square,
                prefixes: [[0; 7]; 8],
                radices: [1; 8],
                count: 0,
            };
            let mut product = 1;
            for (direction, (dx, dy)) in [
                (0, -1),
                (0, 1),
                (-1, 0),
                (1, 0),
                (-1, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
            ]
            .into_iter()
            .enumerate()
            {
                let (mut x, mut y) = (square as i32 % 8 + dx, square as i32 / 8 + dy);
                let mut prefix = 0;
                let mut length = 0;
                while (0..8).contains(&x) && (0..8).contains(&y) {
                    let bit = 1_u64 << (8 * y + x);
                    if opponent & bit == 0 {
                        break;
                    }
                    block.prefixes[direction][length] = prefix;
                    prefix |= bit;
                    length += 1;
                    x += dx;
                    y += dy;
                }
                block.radices[direction] = length.max(1);
                product *= block.radices[direction];
            }
            block.count = product - 1; // The all-zero unflip is not a move.
            if block.count != 0 {
                blocks.push(block);
            }
        }
        blocks
    }

    fn decode(&self, position: Position, index: usize) -> Position {
        assert!(index < self.count);
        let mut mixed = index + 1;
        let mut flips = 0;
        for direction in 0..8 {
            let digit = mixed % self.radices[direction];
            mixed /= self.radices[direction];
            flips |= self.prefixes[direction][digit];
        }
        let Board { player, opponent } = position.board;
        Position {
            board: Board::new(opponent & !(flips | (1_u64 << self.square)), player | flips),
            side_to_move: position.side_to_move.opposite(),
        }
    }
}

fn decode_global(
    blocks: &[SquareChoices],
    position: Position,
    mut index: usize,
) -> (Position, usize) {
    for block in blocks {
        if index < block.count {
            return (block.decode(position, index), block.square);
        }
        index -= block.count;
    }
    unreachable!("the sampled index is bounded by the sum of block sizes")
}

fn retainable(position: Position, table: &ForwardTable, constraints: &ReverseConstraints) -> bool {
    if position.board.popcount() <= table.discs() {
        return table.contains(position);
    }
    check_virtual_edge_patterns(position.board.player, position.board.opponent)
        && constraints
            .candidates(position)
            .is_some_and(|mask| mask != 0)
}

/// Make one ranking group. Every demonstrated position must be on a supplied,
/// independently replayed source history. The caller supplies both positions
/// around a forced pass when both have one fewer stone than `goal`.
pub fn sample_training_group(
    goal: Position,
    observed: &[Position],
    table: &ForwardTable,
    comparisons: usize,
    max_draws: usize,
    rng: &mut impl Rng,
) -> io::Result<TrainingGroup> {
    if table.start() != Position::initial() || goal.board.popcount() <= table.discs() {
        return Err(io::Error::other(
            "ranking examples require the standard start and a goal above the frontier",
        ));
    }
    table.validate_goal(goal)?;
    let constraints = ReverseConstraints::new(table, goal);
    let mask = constraints.candidates(goal).ok_or_else(|| {
        io::Error::other("a demonstrated legal goal failed a necessary condition")
    })?;
    let blocks = SquareChoices::from_position(goal, mask);
    let total = blocks.iter().map(|block| block.count).sum::<usize>();
    let mut result = TrainingGroup {
        candidates: Vec::new(),
        placement_combinations: total,
        sampled_placements: 0,
        examined_candidates: 0,
        filtered_candidates: 0,
        draw_cap_reached: false,
    };
    let mut positions = HashSet::new();
    for &position in observed {
        if position.board.popcount() + 1 != goal.board.popcount() {
            return Err(io::Error::other(
                "observed predecessor has the wrong stone count",
            ));
        }
        if retainable(position, table, &constraints) && positions.insert(position) {
            result.candidates.push((position, true));
        }
    }
    if result.candidates.is_empty() || comparisons == 0 {
        return Ok(result);
    }

    // Sampling without replacement uses a sparse Fisher-Yates permutation of
    // all mixed-radix indices. It costs O(max_draws) even for a huge branching
    // factor and has no bias towards the iterator's early squares or prefixes.
    let mut swaps = std::collections::HashMap::new();
    let mut accepted = 0;
    let limit = max_draws.min(total);
    for drawn in 0..limit {
        let remaining = total - drawn;
        let selected = rng.random_range(0..remaining);
        let index = *swaps.get(&selected).unwrap_or(&selected);
        let last = *swaps.get(&(remaining - 1)).unwrap_or(&(remaining - 1));
        swaps.insert(selected, last);
        swaps.remove(&(remaining - 1));
        result.sampled_placements += 1;
        let (previous, square) = decode_global(&blocks, goal, index);
        let verified = replay(previous, &[PlyAction::Play(square)]).map_err(io::Error::other)?;
        if verified.last() != Some(&goal) {
            return Err(io::Error::other("sampled inverse move failed exact replay"));
        }
        for candidate in [Some(previous), pass_predecessor(previous)]
            .into_iter()
            .flatten()
        {
            if candidate != previous {
                let verified = replay(candidate, &[PlyAction::Pass, PlyAction::Play(square)])
                    .map_err(io::Error::other)?;
                if verified.last() != Some(&goal) {
                    return Err(io::Error::other("sampled inverse pass failed exact replay"));
                }
            }
            result.examined_candidates += 1;
            if observed.contains(&candidate) || positions.contains(&candidate) {
                continue;
            }
            if !retainable(candidate, table, &constraints) {
                result.filtered_candidates += 1;
                continue;
            }
            positions.insert(candidate);
            result.candidates.push((candidate, false));
            accepted += 1;
            if accepted == comparisons {
                return Ok(result);
            }
        }
    }
    result.draw_cap_reached = limit < total && accepted < comparisons;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::othello::get_last_moves;
    use crate::search::core::{successors, PlacementPredecessors};
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn indexed_candidates_equal_the_exact_iterator() {
        let mut rng = ChaCha8Rng::seed_from_u64(2026102507);
        for _ in 0..24 {
            let mut position = Position::initial();
            while position.board.popcount() < 48 {
                let next = successors(position);
                if next.is_empty() {
                    break;
                }
                position = next[rng.random_range(0..next.len())].0;
                let mask = get_last_moves(position.board.opponent, position.board.player);
                let blocks = SquareChoices::from_position(position, mask);
                let total = blocks.iter().map(|block| block.count).sum::<usize>();
                let exact = PlacementPredecessors::new(position, mask).collect::<Vec<_>>();
                assert_eq!(total, exact.len());
                for (index, &(expected, action)) in exact.iter().enumerate() {
                    let (actual, square) = decode_global(&blocks, position, index);
                    assert_eq!(actual, expected);
                    assert_eq!(action, PlyAction::Play(square));
                }
            }
        }
    }

    #[test]
    fn known_history_is_observed_and_comparisons_are_not_negative_labels() {
        let table = ForwardTable::new(8, None).unwrap();
        let mut rng = ChaCha8Rng::seed_from_u64(2026102508);
        let mut position = Position::initial();
        for _ in 0..40 {
            let next = successors(position);
            if next.is_empty() {
                break;
            }
            let (goal, action) = next[rng.random_range(0..next.len())];
            if goal.board.popcount() > 10 && matches!(action, PlyAction::Play(_)) {
                let group =
                    sample_training_group(goal, &[position], &table, 7, 128, &mut rng).unwrap();
                assert!(group.sampled_placements <= 128);
                assert!(group.candidates.len() <= 8);
                for &(candidate, observed) in &group.candidates {
                    assert_eq!(observed, candidate == position);
                }
            }
            position = goal;
        }
    }
}
