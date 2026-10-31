//! Necessary conditions shared by the reverse solvers.

use crate::filters::{impossible_edges::check_virtual_edge_patterns, reverse::PlacementFilter};
use crate::othello::{get_last_moves, Position};
use crate::search::{forward::ForwardTable, frontier::ForwardOccupancyIndex};

/// Per-query caches and necessary conditions. A candidate mask constrains only
/// placements: even `Some(0)` must retain a legal reverse pass. `None` proves
/// that the position itself cannot connect to the requested forward frontier.
pub(crate) struct ReverseConstraints {
    frontier: ForwardOccupancyIndex,
    placement: Option<PlacementFilter>,
}

impl ReverseConstraints {
    pub(crate) fn new(table: &ForwardTable, goal: Position) -> Self {
        // The occupancy and seg3 proofs start from the standard opening, not
        // from an arbitrary user-provided position. The exact frontier test
        // remains valid for either kind of start.
        let placement = (table.start() == Position::initial()).then(PlacementFilter::new);
        Self {
            frontier: ForwardOccupancyIndex::new(table, goal),
            placement,
        }
    }

    /// Call only above the meeting boundary, after checking table membership.
    pub(crate) fn candidates(&self, position: Position) -> Option<u64> {
        let occupied = position.board.player | position.board.opponent;
        let required = self.frontier.required_squares(occupied)?;
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

#[cfg(test)]
mod tests {
    use super::ReverseConstraints;
    use crate::filters::{linear_programming::check_lp, FilterResult};
    use crate::io::parse_position;
    use crate::othello::{get_last_moves, Board, Color, Position, CENTER_MASK};
    use crate::record::{replay, PlyAction};
    use crate::search::{
        core::{successors, SearchOutcome},
        dfs::retrospective_search,
        forward::ForwardTable,
        parallel_dfs::retrospective_search_parallel,
        parallel_gbfs::parallel_retrospective_greedy_best_first_search,
        transposition::Btable,
    };

    fn assert_all_engines_reach(start: Position, goal: Position, required_pass: bool) {
        let table = ForwardTable::from_start(start, start.board.popcount(), None).unwrap();
        let mut visited = Btable::new(10_000, 256);
        let outcomes = [
            retrospective_search(goal, &table, &mut visited, usize::MAX).unwrap(),
            retrospective_search_parallel(goal, &table, usize::MAX, 10_000, 2).unwrap(),
            parallel_retrospective_greedy_best_first_search(goal, &table, usize::MAX, false, 2)
                .unwrap(),
            parallel_retrospective_greedy_best_first_search(goal, &table, usize::MAX, true, 2)
                .unwrap(),
        ];
        for outcome in outcomes {
            let SearchOutcome::Found { record, .. } = outcome else {
                panic!("reachable custom-start query was not proved: {outcome:?}");
            };
            assert_eq!(record.start, start);
            assert_eq!(record.goal, goal);
            record.verify().unwrap();
            if required_pass {
                assert!(record.actions.contains(&PlyAction::Pass));
            }
        }
    }

    #[test]
    fn standard_opening_conditions_do_not_leak_into_custom_starts() {
        let initial = Position::initial();
        let start = Position {
            board: initial.board.swapped(),
            ..initial
        };
        let goal = successors(start)[0].0;
        let table = ForwardTable::from_start(start, 4, None).unwrap();
        let constraints = ReverseConstraints::new(&table, goal);
        assert!(constraints.placement.is_none());
        assert_all_engines_reach(start, goal, false);
    }

    #[test]
    fn standard_opening_lp_does_not_reject_a_custom_start_suffix() {
        let start = Position {
            board: Board::new(CENTER_MASK, 1_u64 << 19),
            side_to_move: Color::Black,
        };
        let goal = replay(start, &[PlyAction::Play(11)]).unwrap()[1];
        assert!(matches!(
            check_lp(goal.board.player, goal.board.opponent, false, false).unwrap(),
            FilterResult::Rejected { .. }
        ));
        assert_all_engines_reach(start, goal, false);
    }

    #[test]
    fn forced_pass_and_terminal_suffix_survive_shared_constraints() {
        let start =
            parse_position("OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO w")
                .unwrap();
        let actions = [PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)];
        let trace = replay(start, &actions).unwrap();
        assert_all_engines_reach(start, trace[2], true);
        assert_all_engines_reach(start, trace[3], true);
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
        assert_eq!(
            ReverseConstraints::new(&table, trace[3]).candidates(trace[2]),
            Some(0),
        );
        assert_all_engines_reach(start, trace[3], true);
    }
}
