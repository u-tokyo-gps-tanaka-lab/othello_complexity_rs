use std::collections::HashSet;

use crate::othello::Board;
use crate::search::core::search;

/// 順方向探索の結果をキャッシュする構造体
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
