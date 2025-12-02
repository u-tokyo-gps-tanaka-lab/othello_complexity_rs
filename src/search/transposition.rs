use std::collections::HashSet;

use crate::othello::Board;
use crate::search::core::search;

/// table for forward leaf nodes
pub struct LeafCache {
    searched: HashSet<[u64; 2]>,
    leaf: HashSet<[u64; 2]>,
}

impl LeafCache {
    pub fn new(discs: i32) -> Self {
        let mut searched: HashSet<[u64; 2]> = HashSet::new();
        let mut leafnode: HashSet<[u64; 2]> = HashSet::new();
        let initial = Board::initial();
        search(&initial, &mut searched, &mut leafnode, discs);
        for i in 4..9 {
            let mut ans = vec![];
            for s in &searched {
                if (s[0] | s[1]).count_ones() == i {
                    ans.push(s);
                }
            }
            println!("i={}, ans.len()={}", i, ans.len());
            ans.sort();
            for j in 0..ans.len() {
                println!("{}", Board::new(ans[j][0], ans[j][1]).to_string());
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

    pub fn leaf(&self) -> &HashSet<[u64; 2]> {
        &self.leaf
    }
}

// Transposition table for retrospective search.
// tableをsorted arrayとして持つと存在チェックが二分探索でできる
// hashsetとarrayを組み合わせることで、挿入性能と読み取り性能の両方をそこそこ高く保つ
pub struct Btable {
    cache_size: usize,
    table: Vec<[u64; 2]>,
    cache: HashSet<[u64; 2]>,
}

impl Btable {
    pub fn new(table_size: usize, cache_size: usize) -> Self {
        Btable {
            cache_size: cache_size,
            table: Vec::with_capacity(table_size),
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
    pub fn insert(&mut self, uni: [u64; 2]) -> bool {
        if self.cache.contains(&uni) {
            return false;
        }
        if let Ok(_) = self.table.binary_search(&uni) {
            return false;
        }
        self.cache.insert(uni);
        if self.cache.len() >= self.cache_size {
            if self.table.len() + self.cache.len() > self.table.capacity() {
                self.cache.clear();
                return true;
            }
            let mut c2v: Vec<[u64; 2]> = self.cache.iter().map(|x| *x).collect();
            self.cache.clear();
            c2v.sort();
            let mut i = self.table.len();
            let mut j = c2v.len();
            self.table.resize(i + j, [0u64; 2]);
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
