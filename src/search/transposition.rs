use std::collections::HashSet;

use crate::{othello::Position, search::forward::fwd_search};

/// table for forward leaf nodes
pub struct LeafCache {
    searched: HashSet<Position>,
    leaf: HashSet<Position>,
}

impl LeafCache {
    pub fn new(discs: i32) -> Self {
        let mut searched: HashSet<Position> = HashSet::new();
        let mut leafnode: HashSet<Position> = HashSet::new();
        let initial = Position::initial();
        fwd_search(&initial, &mut searched, &mut leafnode, discs);
        leafnode.extend(searched.iter().copied());
        for i in 4..9 {
            let mut ans = vec![];
            for s in &searched {
                if s.board.popcount() == i {
                    ans.push(s);
                }
            }
            println!("i={}, ans.len()={}", i, ans.len());
            ans.sort();
            for j in 0..ans.len() {
                println!("{}", crate::io::format_position(*ans[j]));
            }
        }
        LeafCache {
            searched,
            leaf: leafnode,
        }
    }

    pub fn searched_count(&self) -> usize {
        self.searched.len()
    }

    pub fn leaf_count(&self) -> usize {
        self.leaf.len()
    }

    pub fn leaf(&self) -> &HashSet<Position> {
        &self.leaf
    }
}

// Transposition table for retrospective search.
// tableをsorted arrayとして持つと存在チェックが二分探索でできる
// hashsetとarrayを組み合わせることで、挿入性能と読み取り性能の両方をそこそこ高く保つ
pub struct Btable {
    table_size: usize,
    cache_size: usize,
    table: Vec<Position>,
    cache: HashSet<Position>,
}

impl Btable {
    pub fn new(table_size: usize, cache_size: usize) -> Self {
        Btable {
            table_size,
            cache_size: cache_size,
            table: Vec::new(),
            cache: HashSet::new(),
        }
    }

    pub fn clear(&mut self) {
        self.table.clear();
        self.cache.clear();
    }

    pub fn len(&self) -> usize {
        let ans = self.cache.len() + self.table.len();
        ans
    }

    /// Returns true if inserted, false if already present.
    /// まずcacheを調べ、なかったらtableを調べる。
    /// cacheのサイズが上限に達したら、マージソートを行いtableに中身を移す。
    pub fn insert(&mut self, uni: Position) -> bool {
        if self.cache.contains(&uni) {
            return false;
        }
        if let Ok(_) = self.table.binary_search(&uni) {
            return false;
        }
        self.cache.insert(uni);
        if self.cache.len() >= self.cache_size {
            if self.table.len() + self.cache.len() > self.table_size {
                self.cache.clear();
                return true;
            }
            let mut c2v: Vec<Position> = self.cache.iter().map(|x| *x).collect();
            self.cache.clear();
            c2v.sort();
            let mut i = self.table.len();
            let mut j = c2v.len();
            // Reserve only this merge; resize must not grow capacity geometrically
            // beyond the configured table limit.
            self.table.reserve_exact(j);
            self.table.resize(i + j, uni);
            for k in (0..(i + j)).rev() {
                if j == 0 || (i > 0 && self.table[i - 1] >= c2v[j - 1]) {
                    self.table[k] = self.table[i - 1];
                    i -= 1;
                } else {
                    self.table[k] = c2v[j - 1];
                    j -= 1;
                }
            }
        }
        return true;
    }
}
