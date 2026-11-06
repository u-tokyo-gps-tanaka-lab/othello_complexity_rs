//! Necessary conditions shared before enumerating placement predecessors.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::filters::{
    occupancy::{check_occupancy, occupancy_order},
    seg3::{check_seg3_more_with_geometry, seg3_geometry, Seg3Geometry},
};
use crate::hash::CustomHash;
use crate::othello::{get_last_moves, Color, Position, CENTER_MASK};
use crate::record::GoalMatch;

const GEOMETRY_CACHE_CAPACITY: usize = 4096;

enum OccupancyGeometry {
    Impossible,
    Viable { removable: u64, seg3: Seg3Geometry },
}

impl OccupancyGeometry {
    fn new(occupied: u64) -> Self {
        if !check_occupancy(occupied) {
            return Self::Impossible;
        }
        let order = occupancy_order(occupied);
        Self::Viable {
            removable: removable_squares(occupied, &order),
            seg3: seg3_geometry(occupied, &order),
        }
    }
}

struct GeometryCache {
    entries: HashMap<u64, Arc<OccupancyGeometry>, CustomHash>,
    insertion_order: VecDeque<u64>,
    capacity: usize,
}

/// Per-query sharing of color-independent pruning data, including failed
/// occupancy checks. The cache retains at most 4096 entries; FIFO eviction
/// changes only performance because every missing entry is recomputed exactly.
/// A worker may briefly retain an Arc to an evicted entry while checking colors.
pub(crate) struct PlacementFilter {
    cache: Mutex<GeometryCache>,
    goal_match: GoalMatch,
}

impl PlacementFilter {
    pub(crate) fn new(goal_match: GoalMatch) -> Self {
        Self::with_capacity(GEOMETRY_CACHE_CAPACITY, goal_match)
    }

    fn with_capacity(capacity: usize, goal_match: GoalMatch) -> Self {
        assert!(capacity > 0);
        Self {
            cache: Mutex::new(GeometryCache {
                entries: HashMap::with_capacity_and_hasher(capacity, CustomHash::default()),
                insertion_order: VecDeque::with_capacity(capacity),
                capacity,
            }),
            goal_match,
        }
    }

    fn geometry(&self, occupied: u64) -> Arc<OccupancyGeometry> {
        {
            let cache = self
                .cache
                .lock()
                .expect("placement filter cache mutex poisoned");
            if let Some(geometry) = cache.entries.get(&occupied) {
                return Arc::clone(geometry);
            }
        }
        // Fixed-point occupancy analysis runs outside the lock. Another worker
        // can compute the same entry concurrently, so check again before insert.
        let computed = Arc::new(OccupancyGeometry::new(occupied));
        let mut cache = self
            .cache
            .lock()
            .expect("placement filter cache mutex poisoned");
        if let Some(geometry) = cache.entries.get(&occupied) {
            return Arc::clone(geometry);
        }
        if cache.entries.len() == cache.capacity {
            let oldest = cache
                .insertion_order
                .pop_front()
                .expect("a full cache has an oldest entry");
            cache.entries.remove(&oldest);
        }
        cache.entries.insert(occupied, Arc::clone(&computed));
        cache.insertion_order.push_back(occupied);
        computed
    }

    /// `None` rejects the position; `Some(mask)` retains its viable placement
    /// squares. In particular, `Some(0)` must still allow a legal reverse pass.
    pub(crate) fn candidates(&self, position: Position) -> Option<u64> {
        let board = position.board;
        let geometry = self.geometry(board.player | board.opponent);
        let OccupancyGeometry::Viable { removable, seg3 } = geometry.as_ref() else {
            return None;
        };
        if !centers_match(position, seg3.immutable_centers(), self.goal_match) {
            return None;
        }
        if !check_seg3_more_with_geometry(board.player, board.opponent, seg3) {
            return None;
        }
        Some(removable & get_last_moves(board.opponent, board.player))
    }
}

fn centers_match(position: Position, immutable: u64, goal_match: GoalMatch) -> bool {
    let initial = Position::initial().board;
    match goal_match {
        GoalMatch::Exact => {
            let black = match position.side_to_move {
                Color::Black => position.board.player,
                Color::White => position.board.opponent,
            };
            (black ^ initial.player) & immutable == 0
        }
        GoalMatch::EquivalenceClass => {
            let player = position.board.player;
            (player ^ initial.player) & immutable == 0
                || (player ^ initial.opponent) & immutable == 0
        }
    }
}

/// Return the noncentral occupied squares whose removal leaves an explainable occupancy.
///
/// `order[q]` is the closure after removing q, with q itself restored. Thus
/// `order[q] == occupied` is equivalent to `check_occupancy(occupied & !(1 << q))`
/// for a noncentral occupied q. All unflip combinations for q have that same
/// occupancy, so a failed test rejects their entire Cartesian product.
fn removable_squares(occupied: u64, order: &[u64; 64]) -> u64 {
    let mut remaining = occupied & !CENTER_MASK;
    let mut removable = 0;
    while remaining != 0 {
        let square = remaining.trailing_zeros() as usize;
        let bit = 1_u64 << square;
        remaining &= remaining - 1;
        if order[square] == occupied {
            removable |= bit;
        }
    }
    removable
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::othello::BoardSymmetry;
    use crate::record::PlyAction;
    use crate::search::moves::successors;

    #[test]
    fn removable_mask_matches_separate_occupancy_checks() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x0cc0_5eed);
        for _ in 0..512 {
            let occupied = rng.random::<u64>() | CENTER_MASK;
            let actual = removable_squares(occupied, &occupancy_order(occupied));
            let mut remaining = occupied & !CENTER_MASK;
            let mut expected = 0;
            while remaining != 0 {
                let bit = remaining.isolate_lowest_one();
                remaining &= remaining - 1;
                if check_occupancy(occupied & !bit) {
                    expected |= bit;
                }
            }
            assert_eq!(actual, expected, "occupied={occupied:#018x}");
        }
    }

    fn assert_last_placement_retained(
        placement_filter: &PlacementFilter,
        position: Position,
        action: PlyAction,
    ) {
        let candidates = placement_filter
            .candidates(position)
            .expect("a legal successor must pass the necessary conditions");
        if let PlyAction::Play(square) = action {
            assert_ne!(
                candidates & (1_u64 << square),
                0,
                "true preceding placement {square} rejected for {position:?}"
            );
        }
    }

    #[test]
    fn legal_transitions_and_complete_games_retain_the_last_placement() {
        let placement_filter = PlacementFilter::new(GoalMatch::Exact);
        let mut seen = HashSet::new();
        let mut open = vec![Position::initial()];
        while let Some(position) = open.pop() {
            if !seen.insert(position) {
                continue;
            }
            for (next, action) in successors(position) {
                if next.board.popcount() <= 8 {
                    assert_last_placement_retained(&placement_filter, next, action);
                    open.push(next);
                }
            }
        }

        let placement_filter = PlacementFilter::new(GoalMatch::Exact);
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x1a57_5eed);
        let mut passes = 0;
        for _ in 0..64 {
            let mut position = Position::initial();
            loop {
                let next = successors(position);
                if next.is_empty() {
                    break;
                }
                let (next, action) = next[rng.random_range(0..next.len())];
                assert_last_placement_retained(&placement_filter, next, action);
                if action == PlyAction::Pass {
                    passes += 1;
                }
                position = next;
            }
        }
        assert!(passes > 0, "the deterministic corpus must cover passes");
    }

    #[test]
    fn immutable_centers_distinguish_exact_and_equivalence_opening_alignments() {
        let occupied = CENTER_MASK | (1_u64 << 19);
        let mirrored = BoardSymmetry::MirrorHorizontal.apply_position(Position::initial());
        let cases = [
            (
                "black monochrome centres",
                Position {
                    board: crate::othello::Board::new(occupied, 0),
                    side_to_move: Color::Black,
                },
                Some(true),
                None,
                None,
            ),
            (
                "white monochrome centres",
                Position {
                    board: crate::othello::Board::new(0, occupied),
                    side_to_move: Color::White,
                },
                Some(true),
                None,
                None,
            ),
            ("mirrored opening", mirrored, None, None, Some(0)),
        ];
        for (name, position, seg3_expected, exact, equivalence) in cases {
            if let Some(expected) = seg3_expected {
                assert_eq!(
                    crate::filters::seg3::check_seg3_more(
                        position.board.player,
                        position.board.opponent,
                        false,
                    ),
                    expected,
                    "case={name}"
                );
            }
            assert_eq!(
                PlacementFilter::new(GoalMatch::Exact).candidates(position),
                exact,
                "case={name}, goal_match=exact"
            );
            assert_eq!(
                PlacementFilter::new(GoalMatch::EquivalenceClass).candidates(position),
                equivalence,
                "case={name}, goal_match=equivalence"
            );
        }
    }

    #[test]
    fn bounded_geometry_cache_evicts_and_recomputes() {
        const D3: usize = 19;
        const C4: usize = 26;

        let placement_filter = PlacementFilter::with_capacity(2, GoalMatch::Exact);
        let initial = Position::initial();
        let successor_after = |square| {
            successors(initial)
                .into_iter()
                .find_map(|(position, action)| {
                    (action == PlyAction::Play(square)).then_some(position)
                })
                .expect("the opening move must be legal")
        };
        let positions = [
            ("initial", initial, Some(0)),
            ("legal D3", successor_after(D3), Some(1_u64 << D3)),
            ("legal C4", successor_after(C4), Some(1_u64 << C4)),
            (
                "D3 with the wrong absolute color",
                Position {
                    board: crate::othello::Board::new(
                        initial.board.player,
                        initial.board.opponent | 1_u64 << D3,
                    ),
                    side_to_move: Color::Black,
                },
                None,
            ),
            (
                "isolated corner",
                Position {
                    board: crate::othello::Board::new(
                        initial.board.player | 1,
                        initial.board.opponent,
                    ),
                    side_to_move: Color::Black,
                },
                None,
            ),
        ];
        for &(name, position, expected) in &positions {
            assert_eq!(
                placement_filter.candidates(position),
                expected,
                "case={name}"
            );
        }
        {
            let cache = placement_filter.cache.lock().unwrap();
            assert_eq!(cache.entries.len(), 2);
            assert_eq!(cache.insertion_order.len(), 2);
        }
        for &(name, position, expected) in positions.iter().cycle().take(12) {
            assert_eq!(
                placement_filter.candidates(position),
                expected,
                "case={name}"
            );
            let cache = placement_filter.cache.lock().unwrap();
            assert_eq!(cache.entries.len(), 2);
            assert_eq!(cache.insertion_order.len(), 2);
        }
    }

    #[test]
    fn bounded_geometry_cache_keeps_its_limit_under_concurrent_misses() {
        let placement_filter = PlacementFilter::with_capacity(7, GoalMatch::Exact);
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x6d75_7465);
        let positions: Vec<_> = (0..32)
            .map(|_| {
                let occupied = rng.random::<u64>() | CENTER_MASK;
                let player = rng.random::<u64>() & occupied;
                Position {
                    board: crate::othello::Board::new(player, occupied ^ player),
                    side_to_move: crate::othello::Color::Black,
                }
            })
            .collect();
        let reference_filter = PlacementFilter::new(GoalMatch::Exact);
        let expected: Vec<_> = positions
            .iter()
            .map(|&position| reference_filter.candidates(position))
            .collect();
        std::thread::scope(|scope| {
            for offset in 0..4 {
                let positions = &positions;
                let expected = &expected;
                let placement_filter = &placement_filter;
                scope.spawn(move || {
                    for step in 0..64 {
                        let index = (step + offset) % positions.len();
                        let position = positions[index];
                        assert_eq!(placement_filter.candidates(position), expected[index]);
                        let cache = placement_filter.cache.lock().unwrap();
                        assert!(cache.entries.len() <= cache.capacity);
                        assert_eq!(cache.insertion_order.len(), cache.entries.len());
                    }
                });
            }
        });
    }
}
