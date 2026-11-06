//! Necessary conditions shared by the reverse solvers.

use std::io;

use crate::filters::{impossible_edges::check_virtual_edge_patterns, reverse::PlacementFilter};
use crate::othello::{get_last_moves, Board, Position};
use crate::search::forward::{
    frontier::{ForwardOccupancyCache, ForwardOccupancyIndex},
    ForwardTable,
};

/// Per-query caches and necessary conditions. A candidate mask constrains only
/// placements: even `Some(0)` must retain a legal reverse pass. `None` proves
/// that the position itself cannot connect to the requested forward frontier.
pub(crate) struct ReverseConstraints<'a> {
    frontier: &'a ForwardOccupancyIndex,
    frontier_cache: ForwardOccupancyCache,
    placement: Option<PlacementFilter>,
}

impl<'a> ReverseConstraints<'a> {
    pub(crate) fn new(table: &ForwardTable, frontier: &'a ForwardOccupancyIndex) -> Self {
        // The occupancy and seg3 proofs start from the standard opening, not
        // from an arbitrary user-provided position. The exact frontier test
        // remains valid for either kind of start.
        let placement = (table.start() == Position::initial())
            .then(|| PlacementFilter::new(table.goal_match()));
        Self {
            frontier,
            frontier_cache: ForwardOccupancyCache::new(),
            placement,
        }
    }

    /// Call only above the meeting boundary, after checking table membership.
    pub(crate) fn candidates(&self, position: Position) -> Option<u64> {
        let occupied = position.board.player | position.board.opponent;
        let required = self
            .frontier
            .required_squares_cached(&self.frontier_cache, occupied)?;
        let candidates = match &self.placement {
            Some(filter) => {
                // Impossible edge patterns are proved from the standard
                // opening, like the other placement conditions.
                if !check_virtual_edge_patterns(position.board.player, position.board.opponent) {
                    return None;
                }
                filter.candidates(position)?
            }
            None => get_last_moves(position.board.opponent, position.board.player),
        };
        Some(candidates & !required)
    }
}

pub(crate) fn passes_lp(position: Position) -> io::Result<bool> {
    use crate::filters::{linear_programming::check_lp, FilterResult};

    let Board { player, opponent } = position.board;
    let result = check_lp(player, opponent, false)?;
    if let FilterResult::Undetermined { reason } = &result {
        eprintln!(
            "info: LP undetermined for {}: {}; retaining this search node",
            crate::io::format_position(position),
            reason
        );
    }
    Ok(result.passes())
}

pub(crate) fn viable(position: Position) -> io::Result<bool> {
    use crate::filters::{
        impossible_edges::check_virtual_edge_patterns,
        occupancy::{check_occupancy, occupancy_order},
        seg3::check_seg3_more_with_order,
    };
    let Board { player, opponent } = position.board;
    let occupied = player | opponent;
    if !check_occupancy(occupied) || !check_virtual_edge_patterns(player, opponent) {
        return Ok(false);
    }
    let order = occupancy_order(occupied);
    if !check_seg3_more_with_order(player, opponent, &order) {
        return Ok(false);
    }
    passes_lp(position)
}

#[cfg(test)]
mod tests {
    use super::ReverseConstraints;
    use crate::filters::{linear_programming::check_lp, seg3::check_seg3_more, FilterResult};
    use crate::io::parse_position;
    use crate::othello::{get_last_moves, Board, Color, Position, CENTER_MASK};
    use crate::record::{replay, PlyAction};
    use crate::search::{
        engines::{
            bfs::retrospective_bfs, dfs::retrospective_search,
            parallel_dfs::retrospective_search_parallel,
            parallel_gbfs::parallel_retrospective_greedy_best_first_search,
        },
        forward::{frontier::ForwardOccupancyIndex, ForwardTable},
        moves::successors,
        outcome::SearchOutcome,
        transposition::Btable,
    };

    fn assert_all_engines_reach(
        start: Position,
        goal: Position,
        required_pass: bool,
        expected_actions: Option<&[PlyAction]>,
        expected_bfs_nodes: Option<usize>,
    ) {
        let table = ForwardTable::from_start(start, start.board.popcount(), None).unwrap();
        let mut visited = Btable::new(10_000, 256);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        let outcomes = [
            (
                "dfs",
                retrospective_search(goal, &table, &mut visited, usize::MAX).unwrap(),
            ),
            (
                "parallel-dfs",
                pool.install(|| retrospective_search_parallel(goal, &table, usize::MAX, 10_000))
                    .unwrap(),
            ),
            (
                "gbfs",
                parallel_retrospective_greedy_best_first_search(goal, &table, usize::MAX, false, 2)
                    .unwrap(),
            ),
            (
                "gbfs-lp",
                parallel_retrospective_greedy_best_first_search(goal, &table, usize::MAX, true, 2)
                    .unwrap(),
            ),
            (
                "bfs",
                pool.install(|| retrospective_bfs(goal, &table, usize::MAX, false))
                    .unwrap(),
            ),
            (
                "bfs-lp",
                pool.install(|| retrospective_bfs(goal, &table, usize::MAX, true))
                    .unwrap(),
            ),
        ];
        for (engine, outcome) in outcomes {
            let SearchOutcome::Found { record, nodes } = outcome else {
                panic!("{engine}: reachable custom-start query was not proved: {outcome:?}");
            };
            assert_eq!(record.start, start, "{engine}");
            assert_eq!(record.goal, goal, "{engine}");
            record.verify().unwrap();
            if required_pass {
                assert!(record.actions.contains(&PlyAction::Pass), "{engine}");
            }
            if let Some(expected_actions) = expected_actions {
                assert_eq!(record.actions, expected_actions, "{engine}");
            }
            if matches!(engine, "bfs" | "bfs-lp") {
                if let Some(expected_nodes) = expected_bfs_nodes {
                    assert_eq!(nodes, expected_nodes, "{engine}");
                }
            }
        }
    }

    #[test]
    fn custom_starts_skip_standard_pruning_in_every_reverse_engine() {
        let initial = Position::initial();
        let start = Position {
            board: initial.board.swapped(),
            ..initial
        };
        let goal = successors(start)[0].0;
        assert_all_engines_reach(start, goal, false, None, None);

        let start = Position {
            board: Board::new(CENTER_MASK, 1_u64 << 19),
            side_to_move: Color::Black,
        };
        assert!(!check_seg3_more(
            start.board.player,
            start.board.opponent,
            false
        ));
        let (goal, action) = successors(start)
            .into_iter()
            .find(|(_, action)| *action == PlyAction::Play(11))
            .expect("D2 must be legal from the custom start");
        assert_eq!(action, PlyAction::Play(11));
        assert!(matches!(
            check_lp(goal.board.player, goal.board.opponent, false).unwrap(),
            FilterResult::Rejected { .. }
        ));
        assert_all_engines_reach(start, goal, false, Some(&[PlyAction::Play(11)]), Some(1));

        let targeted_table = ForwardTable::from_start(start, 5, Some(goal)).unwrap();
        let targeted = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| retrospective_bfs(goal, &targeted_table, usize::MAX, true))
            .unwrap();
        let SearchOutcome::Found { record, nodes: 1 } = targeted else {
            panic!("targeted BFS did not prove the custom-start query: {targeted:?}");
        };
        assert_eq!(record.start, start);
        assert_eq!(record.goal, goal);
        assert_eq!(record.actions, [PlyAction::Play(11)]);
        assert_eq!(record.verify().unwrap().last(), Some(&goal));
    }

    #[test]
    fn forced_pass_and_terminal_suffix_survive_shared_constraints() {
        let start =
            parse_position("OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO w")
                .unwrap();
        let actions = [PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)];
        let trace = replay(start, &actions).unwrap();
        assert_all_engines_reach(start, trace[2], true, None, None);
        assert_all_engines_reach(start, trace[3], true, None, None);
    }

    #[test]
    fn zero_placement_state_keeps_its_separately_registered_reverse_pass() {
        let empty = (1_u64 << 0) | (1_u64 << 20);
        let black = (1_u64 << 1) | (1_u64 << 19);
        let start = Position {
            board: Board::new(!(empty | black), black),
            side_to_move: Color::White,
        };
        let trace = replay(
            start,
            &[PlyAction::Play(0), PlyAction::Pass, PlyAction::Play(20)],
        )
        .unwrap();
        assert_eq!(
            get_last_moves(trace[2].board.opponent, trace[2].board.player),
            0,
        );
        let table = ForwardTable::from_start(start, 62, Some(trace[3])).unwrap();
        let frontier = ForwardOccupancyIndex::new(&table, trace[3]);
        assert_eq!(
            ReverseConstraints::new(&table, &frontier).candidates(trace[2]),
            Some(0),
        );
        assert_all_engines_reach(start, trace[3], true, None, None);
    }

    #[test]
    fn standard_start_bfs_applies_lp_without_rejecting_reachable_goals() {
        let rejected =
            parse_position("--OO---XOX-O-XX--X-OOOXOXOXXO--OXXXXXXOOX-XOXXO--OOXOXXXOOOX---- b")
                .unwrap();
        assert!(matches!(
            check_lp(rejected.board.player, rejected.board.opponent, false).unwrap(),
            FilterResult::Rejected { .. }
        ));
        let table = ForwardTable::new(4, Some(rejected)).unwrap();
        let frontier = ForwardOccupancyIndex::new(&table, rejected);
        assert!(ReverseConstraints::new(&table, &frontier)
            .candidates(rejected)
            .is_some());
        assert!(matches!(
            retrospective_bfs(rejected, &table, 0, false).unwrap(),
            SearchOutcome::Unknown { nodes: 1 }
        ));
        assert!(matches!(
            retrospective_bfs(rejected, &table, usize::MAX, true).unwrap(),
            SearchOutcome::NotFound { nodes: 0 }
        ));

        let actions = [
            PlyAction::Play(19),
            PlyAction::Play(20),
            PlyAction::Play(13),
            PlyAction::Play(12),
        ];
        let reachable = *replay(Position::initial(), &actions)
            .unwrap()
            .last()
            .unwrap();
        let table = ForwardTable::new(4, Some(reachable)).unwrap();
        let outcome = retrospective_bfs(reachable, &table, usize::MAX, true).unwrap();
        let SearchOutcome::Found { record, .. } = outcome else {
            panic!("LP-pruned BFS rejected a reachable goal: {outcome:?}");
        };
        record.verify().unwrap();
    }
}
