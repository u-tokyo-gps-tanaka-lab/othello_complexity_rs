//! Necessary occupancy conditions supplied by the exact forward frontier.

use crate::othello::{Position, CENTER_MASK};
use crate::search::forward::{compatible, ForwardTable};
use std::sync::Mutex;

const CACHE_SIZE: usize = 4096;
const TREE_THRESHOLD: usize = 64;
type CacheEntry = Option<(u64, Option<u64>)>;

#[derive(Clone, Copy)]
struct OccupancyNode {
    common: u64,
    possible: u64,
    // A leaf has common == possible, so one of the two mask tests always
    // answers its query before these unused child indices can be accessed.
    children: [usize; 2],
}

/// A compressed binary tree over sorted, distinct occupancy masks. Unvarying
/// bits consume no nodes; the greatest differing bit partitions each family.
/// Nodes are stored in postorder, making the root the final element.
struct OccupancyTree {
    nodes: Vec<OccupancyNode>,
}

impl OccupancyTree {
    fn new(occupancies: &[u64]) -> Self {
        debug_assert!(occupancies.windows(2).all(|pair| pair[0] < pair[1]));
        let mut tree = Self {
            nodes: Vec::with_capacity(occupancies.len().saturating_mul(2).saturating_sub(1)),
        };
        if !occupancies.is_empty() {
            tree.append_family(occupancies);
        }
        tree
    }

    fn append_family(&mut self, family: &[u64]) -> usize {
        let node = if family.len() == 1 {
            OccupancyNode {
                common: family[0],
                possible: family[0],
                children: [0, 0],
            }
        } else {
            let difference = family[0] ^ family[family.len() - 1];
            let split_bit = 1_u64 << (63 - difference.leading_zeros());
            // Higher bits are constant throughout this sorted family, so its
            // zero/one runs at split_bit are contiguous and both nonempty.
            let split = family.partition_point(|mask| mask & split_bit == 0);
            debug_assert!(split > 0 && split < family.len());
            let left = self.append_family(&family[..split]);
            let right = self.append_family(&family[split..]);
            OccupancyNode {
                common: self.nodes[left].common & self.nodes[right].common,
                possible: self.nodes[left].possible | self.nodes[right].possible,
                children: [left, right],
            }
        };
        let index = self.nodes.len();
        self.nodes.push(node);
        index
    }

    fn required_squares(&self, occupied: u64) -> Option<u64> {
        let root = self.nodes.len().checked_sub(1)?;
        self.intersection(root, !occupied)
    }

    fn intersection(&self, index: usize, excluded: u64) -> Option<u64> {
        let node = self.nodes[index];
        // Every member contains a forbidden square: no subset can survive.
        if node.common & excluded != 0 {
            return None;
        }
        // No member contains a forbidden square: the whole family survives.
        if node.possible & excluded == 0 {
            return Some(node.common);
        }
        debug_assert_ne!(node.common, node.possible);
        let left = self.intersection(node.children[0], excluded);
        // Any surviving intersection contains node.common. Reaching this
        // lower bound cannot be improved by the other child. For a legal
        // frontier this includes reaching the central four squares alone.
        if left == Some(node.common) {
            return left;
        }
        match (left, self.intersection(node.children[1], excluded)) {
            (Some(a), Some(b)) => Some(a & b),
            (Some(mask), None) | (None, Some(mask)) => Some(mask),
            (None, None) => None,
        }
    }
}

/// Distinct frontier occupancies compatible with one query's goal occupancy.
///
/// If a reverse node with at least `table.discs()` stones is reachable, some
/// exact forward position at that boundary must have an occupancy contained in
/// the node. Its remaining reverse placement steps cannot remove a square
/// shared by every such frontier occupancy before reaching the boundary.
///
/// Callers must use this index only for reverse ancestors of the goal used to
/// construct it and validate that goal against the supplied forward table.
pub(crate) struct ForwardOccupancyIndex {
    occupancies: Vec<u64>,
    tree: Option<OccupancyTree>,
    // A direct-mapped cache stores complete keys. A collision only causes a
    // recomputation; it can never turn a missing frontier into a proof.
    cache: Mutex<Vec<CacheEntry>>,
}

impl ForwardOccupancyIndex {
    pub(crate) fn new(table: &ForwardTable, goal: Position) -> Self {
        let occupancies: Vec<u64> = table
            .positions()
            .filter(|position| position.board.popcount() == table.discs())
            // Apply the goal-specific immutable-color condition before dropping
            // colors. One compatible assignment is enough to retain its mask.
            .filter(|position| compatible(*position, goal))
            .map(|position| position.board.player | position.board.opponent)
            .collect();
        Self::from_occupancies(occupancies)
    }

    fn from_occupancies(mut occupancies: Vec<u64>) -> Self {
        occupancies.sort_unstable();
        occupancies.dedup();
        occupancies.shrink_to_fit();
        let tree = (occupancies.len() >= TREE_THRESHOLD).then(|| OccupancyTree::new(&occupancies));
        Self {
            occupancies,
            tree,
            cache: Mutex::new(vec![None; CACHE_SIZE]),
        }
    }

    /// Return the squares shared by all frontier occupancies contained in
    /// `occupied`, or `None` if no frontier occupancy is contained in it.
    ///
    /// For a nonboundary reverse node, a placement on any returned square is
    /// impossible as its last move: removing it would eliminate every possible
    /// meeting occupancy. Legal reverse passes leave this condition unchanged.
    pub(crate) fn required_squares(&self, occupied: u64) -> Option<u64> {
        let mixed = (occupied ^ (occupied >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        let mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        let slot = ((mixed ^ (mixed >> 31)) as usize) & (CACHE_SIZE - 1);
        {
            let cache = self.cache.lock().expect("frontier cache mutex poisoned");
            if let Some((key, required)) = cache[slot] {
                if key == occupied {
                    return required;
                }
            }
        }
        let required = self.compute_required_squares(occupied);
        self.cache.lock().expect("frontier cache mutex poisoned")[slot] =
            Some((occupied, required));
        required
    }

    fn compute_required_squares(&self, occupied: u64) -> Option<u64> {
        match &self.tree {
            Some(tree) => tree.required_squares(occupied),
            None => self.linear_required_squares(occupied),
        }
    }

    fn linear_required_squares(&self, occupied: u64) -> Option<u64> {
        let mut required = None;
        for &frontier in &self.occupancies {
            if frontier & occupied != frontier {
                continue;
            }
            let common = required.unwrap_or(u64::MAX) & frontier;
            required = Some(common);
            // Every legal frontier contains the central four squares, so no
            // further match can reduce this intersection once only they remain.
            if common == CENTER_MASK {
                break;
            }
        }
        required
    }
}

#[cfg(test)]
mod tests {
    use super::{ForwardOccupancyIndex, OccupancyTree, TREE_THRESHOLD};
    use crate::filters::occupancy::check_occupancy;
    use crate::othello::{Board, Color, Position, CENTER_MASK};
    use crate::search::core::predecessors;
    use crate::search::forward::ForwardTable;
    use rand::{Rng, SeedableRng};
    use std::collections::HashSet;

    fn occupied(position: Position) -> u64 {
        position.board.player | position.board.opponent
    }

    fn all_squares_goal() -> Position {
        Position {
            board: Board::new(u64::MAX, 0),
            side_to_move: Color::Black,
        }
    }

    fn direct_intersection(family: &[u64], occupied: u64) -> Option<u64> {
        family
            .iter()
            .copied()
            .filter(|mask| mask & occupied == *mask)
            .reduce(|a, b| a & b)
    }

    #[test]
    fn occupancy_tree_handles_empty_leaf_and_mixed_subfamilies() {
        assert_eq!(OccupancyTree::new(&[]).required_squares(u64::MAX), None);
        let leaf = CENTER_MASK | (1_u64 << 63);
        let tree = OccupancyTree::new(&[leaf]);
        assert_eq!(tree.required_squares(leaf), Some(leaf));
        assert_eq!(tree.required_squares(u64::MAX), Some(leaf));
        assert_eq!(tree.required_squares(CENTER_MASK), None);
        assert_eq!(tree.required_squares(0), None);

        let high = 1_u64 << 63;
        let next = 1_u64 << 62;
        let family = [
            CENTER_MASK | next | 1,
            CENTER_MASK | high | 1,
            CENTER_MASK | high | 2,
        ];
        let tree = OccupancyTree::new(&family);
        for (mask, expected) in [
            (CENTER_MASK, None),
            (CENTER_MASK | high | 1, Some(CENTER_MASK | high | 1)),
            (CENTER_MASK | high | 3, Some(CENTER_MASK | high)),
            (CENTER_MASK | high | next | 1, Some(CENTER_MASK | 1)),
            (u64::MAX, Some(CENTER_MASK)),
        ] {
            assert_eq!(tree.required_squares(mask), expected);
            assert_eq!(
                tree.required_squares(mask),
                direct_intersection(&family, mask)
            );
        }
    }

    #[test]
    fn small_families_keep_linear_queries_after_deduplication() {
        for size in [0, 1, TREE_THRESHOLD - 1, TREE_THRESHOLD, TREE_THRESHOLD + 1] {
            let masks: Vec<_> = (0..size as u64).map(|bits| CENTER_MASK | bits).collect();
            let mut duplicates = masks.clone();
            duplicates.extend(masks.iter().copied());
            duplicates.reverse();
            let index = ForwardOccupancyIndex::from_occupancies(duplicates);
            assert_eq!(index.occupancies, masks);
            assert_eq!(index.tree.is_some(), size >= TREE_THRESHOLD);
            assert_eq!(
                index.required_squares(u64::MAX),
                direct_intersection(&masks, u64::MAX)
            );
        }
    }

    #[test]
    fn random_occupancy_trees_match_linear_intersections() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xf012_71e2);
        for size in [0, 1, 2, 7, 63, 64, 65, 128, 257] {
            for density in 0..3 {
                let mut family: Vec<_> = (0..size)
                    .map(|_| {
                        let a = rng.random::<u64>();
                        let b = rng.random::<u64>();
                        match density {
                            0 => a & b,
                            1 => a,
                            _ => a | b,
                        }
                    })
                    .collect();
                family.sort_unstable();
                family.dedup();
                // Check the tree's set algebra even without Othello's central
                // square invariant, then compare the production index using it.
                let general_tree = OccupancyTree::new(&family);
                let index = ForwardOccupancyIndex::from_occupancies(
                    family.iter().map(|mask| mask | CENTER_MASK).collect(),
                );
                let forced_tree = OccupancyTree::new(&index.occupancies);
                for query in 0..128 {
                    let mut mask = rng.random::<u64>();
                    if query % 3 == 0 {
                        mask |= rng.random::<u64>();
                    }
                    if !family.is_empty() && query % 3 == 1 {
                        mask |= family[query % family.len()];
                    }
                    assert_eq!(
                        general_tree.required_squares(mask),
                        direct_intersection(&family, mask),
                        "size={size}, density={density}, query={mask:#018x}",
                    );
                    let occupied = mask | CENTER_MASK;
                    let expected = index.linear_required_squares(occupied);
                    assert_eq!(expected, direct_intersection(&index.occupancies, occupied));
                    assert_eq!(forced_tree.required_squares(occupied), expected);
                    assert_eq!(index.compute_required_squares(occupied), expected);
                    assert_eq!(index.required_squares(occupied), expected);
                    assert_eq!(index.required_squares(occupied), expected);
                }
            }
        }
    }

    #[test]
    fn actual_forward_frontiers_match_linear_queries() {
        let table = ForwardTable::new(9, None).unwrap();
        let all = ForwardOccupancyIndex::new(&table, all_squares_goal());
        assert!(
            all.tree.is_some(),
            "the real frontier must exercise the tree path"
        );
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xf04a_9d12);
        let goal_masks = [
            u64::MAX,
            rng.random::<u64>() | rng.random::<u64>() | CENTER_MASK,
            rng.random::<u64>() | CENTER_MASK,
        ];
        for goal_occupied in goal_masks {
            let goal = Position {
                board: Board::new(goal_occupied, 0),
                side_to_move: Color::Black,
            };
            let index = ForwardOccupancyIndex::new(&table, goal);
            let forced_tree = OccupancyTree::new(&index.occupancies);
            for &frontier in &all.occupancies {
                for mask in [
                    goal_occupied,
                    frontier & goal_occupied,
                    (frontier | rng.random::<u64>()) & goal_occupied,
                ] {
                    let expected = index.linear_required_squares(mask);
                    assert_eq!(forced_tree.required_squares(mask), expected);
                    assert_eq!(index.required_squares(mask), expected);
                }
            }
        }
    }

    #[test]
    fn forward_occupancy_keeps_all_shallow_reachable_ancestors() {
        let boundary = ForwardTable::new(6, None).unwrap();
        let reachable = ForwardTable::new(8, None).unwrap();
        for goal in reachable.positions().filter(|p| p.board.popcount() == 8) {
            let index = ForwardOccupancyIndex::new(&boundary, goal);
            let mut seen = HashSet::new();
            let mut open = vec![goal];
            while let Some(node) = open.pop() {
                if !seen.insert(node) {
                    continue;
                }
                let mask = occupied(node);
                let required = index
                    .required_squares(mask)
                    .expect("a reachable ancestor must retain a meeting occupancy");
                if node.board.popcount() == boundary.discs() {
                    assert_eq!(required, mask);
                    continue;
                }
                for (previous, _) in predecessors(node) {
                    if reachable.contains(previous) {
                        let removed = mask & !occupied(previous);
                        assert_eq!(
                            removed & required,
                            0,
                            "pruned a reachable predecessor of {node:?}"
                        );
                        open.push(previous);
                    }
                }
            }
        }
    }

    #[test]
    fn forward_occupancy_removal_condition_matches_direct_frontier_scan() {
        let boundary = ForwardTable::new(6, None).unwrap();
        let reachable = ForwardTable::new(8, None).unwrap();
        let index = ForwardOccupancyIndex::new(&boundary, all_squares_goal());
        for node in reachable.positions().filter(|p| p.board.popcount() >= 6) {
            let mask = occupied(node);
            let matching: Vec<_> = boundary
                .positions()
                .filter(|p| p.board.popcount() == boundary.discs())
                .map(occupied)
                .filter(|frontier| frontier & mask == *frontier)
                .collect();
            let expected = matching.iter().copied().reduce(|a, b| a & b);
            assert_eq!(index.required_squares(mask), expected);
            let required = expected.unwrap();
            let mut squares = mask & !CENTER_MASK;
            while squares != 0 {
                let square = 1_u64 << squares.trailing_zeros();
                squares &= squares - 1;
                let after_removal = mask & !square;
                let any_survivor = matching
                    .iter()
                    .any(|frontier| frontier & after_removal == *frontier);
                assert_eq!(any_survivor, required & square == 0);
            }
        }
    }

    #[test]
    fn forward_occupancy_can_reject_a_colorless_reachable_occupancy() {
        let boundary = ForwardTable::new(6, None).unwrap();
        let index = ForwardOccupancyIndex::new(&boundary, all_squares_goal());
        // D3 and C4 can each be Black's first placement. Neither can be
        // White's reply after the other, despite passing colorless closure.
        let mask = CENTER_MASK | (1_u64 << 19) | (1_u64 << 26);
        assert!(check_occupancy(mask));
        assert_eq!(index.required_squares(mask), None);
    }

    #[test]
    fn forward_occupancy_index_filters_and_deduplicates_goal_masks() {
        let boundary = ForwardTable::new(7, None).unwrap();
        for goal in boundary.positions().filter(|p| p.board.popcount() == 7) {
            let index = ForwardOccupancyIndex::new(&boundary, goal);
            assert_eq!(index.occupancies, vec![occupied(goal)]);
        }
    }

    #[test]
    fn shared_frontier_rejects_goal_colors_that_cannot_change() {
        let boundary = ForwardTable::new(5, None).unwrap();
        let mask = CENTER_MASK | (1_u64 << 19);
        assert!(boundary
            .positions()
            .any(|position| occupied(position) == mask));
        let goal = Position {
            board: Board::new(mask, 0),
            side_to_move: Color::Black,
        };
        let index = ForwardOccupancyIndex::new(&boundary, goal);
        assert!(index.occupancies.is_empty());
        assert_eq!(index.required_squares(mask), None);
    }
}
