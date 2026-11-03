//! Exact position identity for reverse-search visited sets.

use std::collections::HashSet;

use dashmap::DashSet;

use crate::hash::CustomHash;
use crate::othello::{Board, Color, Position};

/// The key includes the relative board, its orientation and the absolute turn.
pub type Visited = HashSet<Position, CustomHash>;

/// An exact concurrent visited set with compact board keys per absolute turn.
pub(crate) struct ConcurrentVisited {
    by_side: [DashSet<Board, CustomHash>; 2],
}

impl Default for ConcurrentVisited {
    fn default() -> Self {
        Self {
            by_side: std::array::from_fn(|_| DashSet::with_hasher(CustomHash::default())),
        }
    }
}

impl ConcurrentVisited {
    pub fn insert(&self, position: Position) -> bool {
        self.by_side[side_index(position.side_to_move)].insert(position.board)
    }

    pub fn contains(&self, position: &Position) -> bool {
        self.by_side[side_index(position.side_to_move)].contains(&position.board)
    }
}

const fn side_index(side_to_move: Color) -> usize {
    match side_to_move {
        Color::Black => 0,
        Color::White => 1,
    }
}

/// A bounded sorted table with a separate insertion cache.
/// When a full cache would exceed the table limit, only that cache is forgotten.
pub struct Btable {
    table_size: usize,
    cache_size: usize,
    table: Vec<Position>,
    cache: HashSet<Position>,
}

impl Btable {
    pub fn new(table_size: usize, cache_size: usize) -> Self {
        Self {
            table_size,
            cache_size,
            table: Vec::new(),
            cache: HashSet::new(),
        }
    }

    /// Forget the previous query while retaining both allocations for reuse.
    pub fn clear(&mut self) {
        self.table.clear();
        self.cache.clear();
    }

    pub fn len(&self) -> usize {
        self.table.len() + self.cache.len()
    }

    /// Return false only when the position is still remembered.
    pub fn insert(&mut self, position: Position) -> bool {
        if self.cache.contains(&position) || self.table.binary_search(&position).is_ok() {
            return false;
        }
        self.cache.insert(position);
        if self.cache.len() >= self.cache_size {
            if self.table.len() + self.cache.len() > self.table_size {
                self.cache.clear();
                return true;
            }
            let mut pending: Vec<Position> = self.cache.iter().copied().collect();
            self.cache.clear();
            pending.sort();
            let mut i = self.table.len();
            let mut j = pending.len();
            // Reserve only this merge so resize cannot geometrically exceed
            // the specified table limit.
            self.table.reserve_exact(j);
            self.table.resize(i + j, position);
            for k in (0..(i + j)).rev() {
                if j == 0 || (i > 0 && self.table[i - 1] >= pending[j - 1]) {
                    self.table[k] = self.table[i - 1];
                    i -= 1;
                } else {
                    self.table[k] = pending[j - 1];
                    j -= 1;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_visited_distinguishes_the_same_board_by_turn() {
        let black = Position::initial();
        let white = Position {
            side_to_move: Color::White,
            ..black
        };
        let visited = ConcurrentVisited::default();

        assert!(visited.insert(black));
        assert!(visited.contains(&black));
        assert!(!visited.contains(&white));
        assert!(visited.insert(white));
        assert!(visited.contains(&white));
    }

    #[test]
    fn concurrent_visited_rejects_a_duplicate_insert() {
        let position = Position::initial();
        let visited = ConcurrentVisited::default();

        assert!(visited.insert(position));
        assert!(!visited.insert(position));
    }
}
