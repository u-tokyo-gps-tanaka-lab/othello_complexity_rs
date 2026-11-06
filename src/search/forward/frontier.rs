//! Necessary occupancy conditions supplied by the forward frontier.

use super::{compatible, ForwardTable, SEQUENTIAL_LAYER_LIMIT};
use crate::othello::{Board, BoardSymmetry, Color, Position, CENTER_MASK};
use crate::record::GoalMatch;
use rayon::prelude::*;
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
/// forward position at that boundary must have an occupancy contained in the
/// node. Equivalence tables store canonical representatives, so this index
/// expands their geometric images before applying the goal condition. The
/// remaining reverse placement steps cannot remove a square shared by every
/// such frontier occupancy before reaching the boundary.
///
/// Callers must use this index only for reverse ancestors of the goal used to
/// construct it and validate that goal against the supplied forward table.
pub(crate) struct ForwardOccupancyIndex {
    occupancies: Vec<u64>,
    tree: Option<OccupancyTree>,
}

pub(crate) struct ForwardOccupancyCache {
    // A direct-mapped cache stores complete keys. A collision only causes a
    // recomputation; it can never turn a missing frontier into a proof.
    entries: Mutex<Vec<CacheEntry>>,
}

impl ForwardOccupancyIndex {
    pub(crate) fn new(table: &ForwardTable, goal: Position) -> Self {
        Self::new_with_limit(table, goal, SEQUENTIAL_LAYER_LIMIT)
    }

    fn new_with_limit(table: &ForwardTable, goal: Position, sequential_layer_limit: usize) -> Self {
        let boundary_len = table.boundary_layer().iter().map(Vec::len).sum::<usize>();
        let sequential = boundary_len <= sequential_layer_limit;
        let mut occupancies = if sequential {
            collect_occupancies_sequential(table, goal)
        } else {
            collect_occupancies_parallel(table, goal)
        };
        if sequential {
            occupancies.sort_unstable();
        } else {
            occupancies.par_sort_unstable();
        }
        Self::from_sorted_occupancies(occupancies)
    }

    #[cfg(test)]
    fn from_occupancies(mut occupancies: Vec<u64>) -> Self {
        occupancies.sort_unstable();
        Self::from_sorted_occupancies(occupancies)
    }

    fn from_sorted_occupancies(mut occupancies: Vec<u64>) -> Self {
        occupancies.dedup();
        occupancies.shrink_to_fit();
        let tree = (occupancies.len() >= TREE_THRESHOLD).then(|| OccupancyTree::new(&occupancies));
        Self { occupancies, tree }
    }

    /// Return the squares shared by all frontier occupancies contained in
    /// `occupied`, or `None` if no frontier occupancy is contained in it.
    ///
    /// For a nonboundary reverse node, a placement on any returned square is
    /// impossible as its last move: removing it would eliminate every possible
    /// meeting occupancy. Legal reverse passes leave this condition unchanged.
    #[cfg(test)]
    pub(crate) fn required_squares(&self, occupied: u64) -> Option<u64> {
        self.compute_required_squares(occupied)
    }

    pub(crate) fn required_squares_cached(
        &self,
        cache: &ForwardOccupancyCache,
        occupied: u64,
    ) -> Option<u64> {
        let mixed = (occupied ^ (occupied >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        let mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        let slot = ((mixed ^ (mixed >> 31)) as usize) & (CACHE_SIZE - 1);
        {
            let entries = cache.entries.lock().expect("frontier cache mutex poisoned");
            if let Some((key, required)) = entries[slot] {
                if key == occupied {
                    return required;
                }
            }
        }
        let required = self.compute_required_squares(occupied);
        cache.entries.lock().expect("frontier cache mutex poisoned")[slot] =
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

fn collect_occupancies_sequential(table: &ForwardTable, goal: Position) -> Vec<u64> {
    match table.goal_match() {
        GoalMatch::Exact => table
            .boundary_layer()
            .iter()
            .enumerate()
            .flat_map(|(side_index, boards)| {
                let side_to_move = if side_index == 0 {
                    Color::Black
                } else {
                    Color::White
                };
                boards
                    .iter()
                    .filter_map(move |&board| exact_occupancy(board, side_to_move, goal))
            })
            .collect(),
        GoalMatch::EquivalenceClass => table.boundary_layer()[0]
            .iter()
            .flat_map(|&board| equivalence_occupancies(board, goal))
            .collect(),
    }
}

fn collect_occupancies_parallel(table: &ForwardTable, goal: Position) -> Vec<u64> {
    match table.goal_match() {
        GoalMatch::Exact => table
            .boundary_layer()
            .par_iter()
            .enumerate()
            .flat_map(|(side_index, boards)| {
                let side_to_move = if side_index == 0 {
                    Color::Black
                } else {
                    Color::White
                };
                boards
                    .par_iter()
                    .filter_map(move |&board| exact_occupancy(board, side_to_move, goal))
            })
            .collect(),
        GoalMatch::EquivalenceClass => table.boundary_layer()[0]
            .par_iter()
            .flat_map_iter(|&board| equivalence_occupancies(board, goal))
            .collect(),
    }
}

fn exact_occupancy(board: Board, side_to_move: Color, goal: Position) -> Option<u64> {
    let position = Position {
        board,
        side_to_move,
    };
    // Apply the goal-specific immutable-color condition before dropping colors.
    compatible(position, goal, GoalMatch::Exact).then_some(board.player | board.opponent)
}

fn equivalence_occupancies(board: Board, goal: Position) -> impl Iterator<Item = u64> {
    let position = Position {
        board,
        side_to_move: Color::Black,
    };
    let occupied = board.player | board.opponent;
    let target_occupied = goal.board.player | goal.board.opponent;
    BoardSymmetry::ALL.into_iter().filter_map(move |symmetry| {
        let image_occupied = symmetry.apply_bits(occupied);
        if image_occupied & target_occupied != image_occupied {
            return None;
        }
        compatible(
            symmetry.apply_position(position),
            goal,
            GoalMatch::EquivalenceClass,
        )
        .then_some(image_occupied)
    })
}

impl ForwardOccupancyCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: Mutex::new(vec![None; CACHE_SIZE]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ForwardOccupancyIndex, OccupancyTree};
    use crate::othello::{Board, BoardSymmetry, Color, Position, CENTER_MASK};
    use crate::record::GoalMatch;
    use crate::search::forward::{compatible, ForwardTable};
    use crate::search::moves::predecessors;
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

    fn linear_frontier_occupancies(table: &ForwardTable, goal: Position) -> Vec<u64> {
        let mut occupancies: Vec<_> = match table.goal_match() {
            GoalMatch::Exact => table
                .boundary()
                .filter(|&position| compatible(position, goal, GoalMatch::Exact))
                .map(occupied)
                .collect(),
            GoalMatch::EquivalenceClass => table
                .boundary()
                .flat_map(|position| {
                    BoardSymmetry::ALL
                        .into_iter()
                        .map(move |symmetry| symmetry.apply_position(position))
                })
                .filter(|&position| compatible(position, goal, GoalMatch::EquivalenceClass))
                .map(occupied)
                .collect(),
        };
        occupancies.sort_unstable();
        occupancies.dedup();
        occupancies
    }

    #[test]
    fn sequential_and_parallel_frontier_occupancies_match_linear_scans() {
        let goal = ForwardTable::new(9, None)
            .unwrap()
            .positions()
            .find(|position| position.board.popcount() == 9)
            .unwrap();
        let exact = ForwardTable::new(8, None).unwrap();
        let equivalence = ForwardTable::for_equivalence(8, goal).unwrap();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(3)
            .build()
            .unwrap();

        for table in [&exact, &equivalence] {
            let expected = linear_frontier_occupancies(table, goal);
            let boundary_len = table.boundary_layer().iter().map(Vec::len).sum::<usize>();
            assert!(boundary_len > 0);
            let sequential = ForwardOccupancyIndex::new_with_limit(table, goal, boundary_len);
            let parallel = pool
                .install(|| ForwardOccupancyIndex::new_with_limit(table, goal, boundary_len - 1));
            assert!(!expected.is_empty());
            assert_eq!(sequential.occupancies, expected);
            assert_eq!(parallel.occupancies, expected);
        }
    }

    #[test]
    fn random_occupancy_trees_match_linear_intersections() {
        for size in [0, 1, 63, 64, 65] {
            let masks: Vec<_> = (0..size as u64).map(|bits| CENTER_MASK | bits).collect();
            let mut duplicates = masks.clone();
            duplicates.extend(masks.iter().copied());
            duplicates.reverse();
            let index = ForwardOccupancyIndex::from_occupancies(duplicates);
            assert_eq!(index.occupancies, masks, "duplicate-family-size={size}");
            assert_eq!(
                index.required_squares(u64::MAX),
                direct_intersection(&masks, u64::MAX),
                "duplicate-family-size={size}"
            );
        }

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
                }
            }
        }

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
    fn forward_occupancy_index_filters_goal_masks() {
        struct Case<'a> {
            name: String,
            table: &'a ForwardTable,
            goal: Position,
            expected: Vec<u64>,
            required: Option<(u64, Option<u64>)>,
        }

        let seven_disc_boundary = ForwardTable::new(7, None).unwrap();
        let five_disc_boundary = ForwardTable::new(5, None).unwrap();
        let mask = CENTER_MASK | (1_u64 << 19);
        assert!(five_disc_boundary
            .positions()
            .any(|position| occupied(position) == mask));
        let color_incompatible_goal = Position {
            board: Board::new(mask, 0),
            side_to_move: Color::Black,
        };

        let mut cases: Vec<_> = seven_disc_boundary
            .positions()
            .filter(|position| position.board.popcount() == 7)
            .enumerate()
            .map(|(index, goal)| Case {
                name: format!("seven-disc-{index}"),
                table: &seven_disc_boundary,
                goal,
                expected: vec![occupied(goal)],
                required: None,
            })
            .collect();
        cases.push(Case {
            name: "color-incompatible".to_owned(),
            table: &five_disc_boundary,
            goal: color_incompatible_goal,
            expected: Vec::new(),
            required: Some((mask, None)),
        });

        for case in cases {
            let index = ForwardOccupancyIndex::new(case.table, case.goal);
            assert_eq!(index.occupancies, case.expected, "{}", case.name);
            if let Some((query, expected)) = case.required {
                assert_eq!(index.required_squares(query), expected, "{}", case.name);
            }
        }
    }
}
