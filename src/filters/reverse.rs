//! Necessary conditions shared before enumerating placement predecessors.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::filters::{
    occupancy::{check_occupancy, occupancy_order},
    seg3::{check_seg3_more_with_geometry, seg3_geometry, Seg3Geometry},
};
use crate::hash::CustomHash;
use crate::othello::{get_last_moves, Color, Position, CENTER_MASK};

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
            removable: removable_squares(occupied, &order, u64::MAX),
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
}

impl PlacementFilter {
    pub(crate) fn new() -> Self {
        Self::with_capacity(GEOMETRY_CACHE_CAPACITY)
    }

    fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self {
            cache: Mutex::new(GeometryCache {
                entries: HashMap::with_capacity_and_hasher(capacity, CustomHash::default()),
                insertion_order: VecDeque::with_capacity(capacity),
                capacity,
            }),
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
        let black = match position.side_to_move {
            Color::Black => board.player,
            Color::White => board.opponent,
        };
        let initial_black = Position::initial().board.player;
        if (black ^ initial_black) & seg3.immutable_centers() != 0 {
            return None;
        }
        if !check_seg3_more_with_geometry(board.player, board.opponent, seg3) {
            return None;
        }
        Some(removable & get_last_moves(board.opponent, board.player))
    }
}

/// Retain candidate squares whose removal leaves an explainable occupancy.
///
/// `order[q]` is the closure after removing q, with q itself restored. Thus
/// `order[q] == occupied` is equivalent to `check_occupancy(occupied & !(1 << q))`
/// for a noncentral occupied q. All unflip combinations for q have that same
/// occupancy, so a failed test rejects their entire Cartesian product.
fn removable_squares(occupied: u64, order: &[u64; 64], candidates: u64) -> u64 {
    let mut remaining = occupied & !CENTER_MASK & candidates;
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

/// Return viable last-placement squares after sharing one occupancy analysis
/// with the existing seg3 color test. `None` rejects the position itself.
///
/// `Some(0)` rejects all placement predecessors, but does not reject a legal
/// pass predecessor. Pass exploration must remain separate from this mask.
#[cfg(test)]
fn placement_candidates(position: Position) -> Option<u64> {
    let board = position.board;
    let occupied = board.player | board.opponent;
    if !check_occupancy(occupied) {
        return None;
    }
    let order = occupancy_order(occupied);
    let geometry = seg3_geometry(occupied, &order);
    let black = match position.side_to_move {
        Color::Black => board.player,
        Color::White => board.opponent,
    };
    let initial_black = Position::initial().board.player;
    if (black ^ initial_black) & geometry.immutable_centers() != 0
        || !check_seg3_more_with_geometry(board.player, board.opponent, &geometry)
    {
        return None;
    }
    Some(removable_squares(
        occupied,
        &order,
        get_last_moves(board.opponent, board.player),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::record::PlyAction;
    use crate::search::core::successors;

    #[test]
    fn removable_mask_matches_separate_occupancy_checks() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x0cc0_5eed);
        for _ in 0..512 {
            let occupied = rng.random::<u64>() | CENTER_MASK;
            let actual = removable_squares(occupied, &occupancy_order(occupied), u64::MAX);
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
    fn every_legal_transition_through_eight_discs_retains_its_last_placement() {
        let placement_filter = PlacementFilter::new();
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
    }

    #[test]
    fn full_legal_games_retain_last_placements_including_after_passes() {
        let placement_filter = PlacementFilter::new();
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
    fn immutable_center_rejects_a_wrong_absolute_color_accepted_by_seg3() {
        let occupied = CENTER_MASK | (1_u64 << 19);
        for side_to_move in [Color::Black, Color::White] {
            let board = match side_to_move {
                Color::Black => crate::othello::Board::new(occupied, 0),
                Color::White => crate::othello::Board::new(0, occupied),
            };
            let position = Position {
                board,
                side_to_move,
            };
            assert!(crate::filters::seg3::check_seg3_more(
                board.player,
                board.opponent,
                false,
            ));
            assert!(PlacementFilter::new().candidates(position).is_none());
        }
    }

    #[test]
    fn bounded_geometry_cache_preserves_candidates_for_color_variants() {
        let placement_filter = PlacementFilter::new();
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x6361_6368);
        for _ in 0..64 {
            let a = rng.random::<u64>();
            let b = rng.random::<u64>();
            for occupied in [
                (a & b) | CENTER_MASK,
                a | CENTER_MASK,
                a | b | CENTER_MASK,
                u64::MAX,
                CENTER_MASK,
            ] {
                for _ in 0..8 {
                    let player = rng.random::<u64>() & occupied;
                    let position = Position {
                        board: crate::othello::Board::new(player, occupied ^ player),
                        side_to_move: crate::othello::Color::Black,
                    };
                    assert_eq!(
                        placement_filter.candidates(position),
                        placement_candidates(position),
                        "position={position:?}"
                    );
                }
                assert!(Arc::ptr_eq(
                    &placement_filter.geometry(occupied),
                    &placement_filter.geometry(occupied)
                ));
            }
        }
    }

    #[test]
    fn bounded_geometry_cache_evicts_and_recomputes() {
        const D3: usize = 19;
        const C4: usize = 26;

        let placement_filter = PlacementFilter::with_capacity(2);
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
            (initial, Some(0)),
            (successor_after(D3), Some(1_u64 << D3)),
            (successor_after(C4), Some(1_u64 << C4)),
        ];
        let masks = positions.map(|(position, _)| position.board.player | position.board.opponent);
        for &(position, expected) in &positions {
            assert_eq!(placement_filter.candidates(position), expected);
        }
        {
            let cache = placement_filter.cache.lock().unwrap();
            assert_eq!(cache.entries.len(), 2);
            assert!(!cache.entries.contains_key(&masks[0]));
            assert!(cache.entries.contains_key(&masks[1]));
            assert!(cache.entries.contains_key(&masks[2]));
        }
        for &(position, expected) in positions.iter().cycle().take(12) {
            assert_eq!(placement_filter.candidates(position), expected);
            let cache = placement_filter.cache.lock().unwrap();
            assert_eq!(cache.entries.len(), 2);
            assert_eq!(cache.insertion_order.len(), 2);
        }
    }

    #[test]
    fn bounded_geometry_cache_keeps_its_limit_under_concurrent_misses() {
        let placement_filter = PlacementFilter::with_capacity(7);
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
        std::thread::scope(|scope| {
            for offset in 0..4 {
                let positions = &positions;
                let placement_filter = &placement_filter;
                scope.spawn(move || {
                    for step in 0..64 {
                        let position = positions[(step + offset) % positions.len()];
                        assert_eq!(
                            placement_filter.candidates(position),
                            placement_candidates(position)
                        );
                        let cache = placement_filter.cache.lock().unwrap();
                        assert!(cache.entries.len() <= cache.capacity);
                        assert_eq!(cache.insertion_order.len(), cache.entries.len());
                    }
                });
            }
        });
    }
}
