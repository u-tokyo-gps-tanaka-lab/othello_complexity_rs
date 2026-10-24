//! Reverse search is checked against the independent array-based rules oracle.
//! The expected classifications do not come from a previous search version.

mod support;

use std::collections::HashSet;

use othello_complexity_rs::othello::{BoardSymmetry, Color, Position};
use othello_complexity_rs::record::{GoalMatch, PlyAction, PositionTransform};
use othello_complexity_rs::search::{
    core::{predecessors, SearchOutcome},
    dfs::retrospective_search,
    forward::ForwardTable,
    move_ordering::retrospective_search_move_ordering,
    parallel_dfs::retrospective_search_parallel,
    parallel_gbfs::parallel_retrospective_greedy_best_first_search,
    transposition::Btable,
};

use support::{enumerate, middle_pass_start, query_targets, ArrayPosition};

fn targets_with_turn_changes(reachable: &HashSet<ArrayPosition>) -> Vec<ArrayPosition> {
    let mut targets = query_targets(reachable);
    // query_targets exchanges colors and turn together. Also change only the
    // turn while keeping the absolute colors fixed, which is a different query.
    let opposite_turns: Vec<_> = targets.iter().map(|goal| goal.opposite_turn()).collect();
    targets.extend(opposite_turns);
    targets.sort_by_key(|goal| goal.position());
    targets.dedup();
    targets
}

fn verify_outcome(goal: ArrayPosition, expected: bool, outcome: SearchOutcome) {
    match outcome {
        SearchOutcome::Found { record, .. } => {
            assert!(expected, "unexpected reachable query: {goal:?}");
            let start = ArrayPosition::initial();
            assert_eq!(record.start, start.position());
            assert_eq!(record.goal, goal.position());
            assert_eq!(record.goal_match, GoalMatch::Exact);
            assert_eq!(record.transform, PositionTransform::IDENTITY);
            let independent_trace = start
                .replay(&record.actions)
                .expect("the certificate must obey the independent rules");
            assert_eq!(independent_trace.last(), Some(&goal));
            let independent_positions: Vec<_> = independent_trace
                .into_iter()
                .map(ArrayPosition::position)
                .collect();
            assert_eq!(record.verify().unwrap(), independent_positions);
        }
        SearchOutcome::NotFound { .. } => {
            assert!(!expected, "reachable oracle query was rejected: {goal:?}");
        }
        SearchOutcome::Unknown { nodes } => {
            panic!("unbounded shallow query returned UNKNOWN after {nodes} nodes: {goal:?}");
        }
    }
}

#[test]
fn dfs_and_gbfs_match_all_eight_disc_oracle_queries() {
    let reachable = enumerate(ArrayPosition::initial(), 8);
    let targets = targets_with_turn_changes(&reachable);
    assert!(targets.iter().any(|goal| !reachable.contains(goal)));
    assert!(targets.iter().any(|goal| reachable.contains(goal)));
    let table = ForwardTable::new(6, None).unwrap();
    let mut visited = Btable::new(10_000, 256);
    for goal in targets {
        let expected = reachable.contains(&goal);
        for ordered in [false, true] {
            visited.clear();
            let outcome = if ordered {
                retrospective_search_move_ordering(
                    goal.position(),
                    &table,
                    &mut visited,
                    usize::MAX,
                )
            } else {
                retrospective_search(goal.position(), &table, &mut visited, usize::MAX)
            }
            .unwrap();
            verify_outcome(goal, expected, outcome);
        }
        verify_outcome(
            goal,
            expected,
            parallel_retrospective_greedy_best_first_search(
                goal.position(),
                &table,
                usize::MAX,
                false,
                1,
            )
            .unwrap(),
        );
    }
}

#[test]
fn searches_match_all_seven_disc_oracle_queries_from_four_disc_boundary() {
    let reachable = enumerate(ArrayPosition::initial(), 7);
    let targets = targets_with_turn_changes(&reachable);
    assert!(targets.iter().any(|goal| !reachable.contains(goal)));
    assert!(targets.iter().any(|goal| reachable.contains(goal)));
    let table = ForwardTable::new(4, None).unwrap();
    let mut visited = Btable::new(10_000, 256);
    for goal in targets {
        let expected = reachable.contains(&goal);
        for ordered in [false, true] {
            visited.clear();
            let outcome = if ordered {
                retrospective_search_move_ordering(
                    goal.position(),
                    &table,
                    &mut visited,
                    usize::MAX,
                )
            } else {
                retrospective_search(goal.position(), &table, &mut visited, usize::MAX)
            }
            .unwrap();
            verify_outcome(goal, expected, outcome);
        }
        verify_outcome(
            goal,
            expected,
            retrospective_search_parallel(goal.position(), &table, usize::MAX, 100_000, 2).unwrap(),
        );
        verify_outcome(
            goal,
            expected,
            parallel_retrospective_greedy_best_first_search(
                goal.position(),
                &table,
                usize::MAX,
                false,
                1,
            )
            .unwrap(),
        );
    }
}

#[test]
fn targeted_tables_and_parallel_searches_match_representative_oracle_queries() {
    let reachable = enumerate(ArrayPosition::initial(), 8);
    let mut bases: Vec<_> = reachable
        .iter()
        .copied()
        .filter(|goal| goal.discs() == 8)
        .collect();
    bases.sort_by_key(|goal| goal.position());
    let mut goals = Vec::new();
    for index in [0, bases.len() / 3, bases.len() * 2 / 3, bases.len() - 1] {
        let goal = bases[index];
        goals.extend([
            goal,
            goal.opposite_turn(),
            goal.color_exchanged(),
            goal.transformed(BoardSymmetry::Rotate90),
        ]);
    }
    goals.sort_by_key(|goal| goal.position());
    goals.dedup();
    assert!(goals.iter().any(|goal| !reachable.contains(goal)));
    let shared_table = ForwardTable::new(6, None).unwrap();
    let mut visited = Btable::new(10_000, 256);
    for goal in goals {
        let expected = reachable.contains(&goal);
        let targeted_table = ForwardTable::new(6, Some(goal.position())).unwrap();
        for table in [&shared_table, &targeted_table] {
            visited.clear();
            verify_outcome(
                goal,
                expected,
                retrospective_search(goal.position(), table, &mut visited, usize::MAX).unwrap(),
            );
            verify_outcome(
                goal,
                expected,
                retrospective_search_parallel(goal.position(), table, usize::MAX, 10_000, 2)
                    .unwrap(),
            );
            for threads in [1, 3] {
                verify_outcome(
                    goal,
                    expected,
                    parallel_retrospective_greedy_best_first_search(
                        goal.position(),
                        table,
                        usize::MAX,
                        false,
                        threads,
                    )
                    .unwrap(),
                );
            }
        }
    }
}

fn as_oracle(position: Position) -> ArrayPosition {
    // This is only a representation conversion. All move and capture decisions
    // below use the array oracle, not production move generation or replay.
    ArrayPosition::from_relative(
        &position.board.to_string(),
        position.side_to_move == Color::Black,
    )
}

fn verify_all_predecessor_edges(goal: ArrayPosition) {
    let previous = predecessors(goal.position());
    let mut unique = HashSet::new();
    for (position, action) in previous {
        let action_key = match action {
            PlyAction::Play(square) => square,
            PlyAction::Pass => 64,
        };
        assert!(
            unique.insert((position, action_key)),
            "duplicate predecessor edge"
        );
        let actual = as_oracle(position)
            .apply(action)
            .expect("every enumerated predecessor must have a legal forward edge");
        assert_eq!(actual, goal);
    }
}

#[test]
fn predecessor_enumeration_matches_independent_legal_edges() {
    let reachable = enumerate(ArrayPosition::initial(), 8);
    for &source in &reachable {
        let moves = source.moves();
        let actions: Vec<_> = if moves.is_empty() {
            source
                .apply(PlyAction::Pass)
                .ok()
                .map(|_| PlyAction::Pass)
                .into_iter()
                .collect()
        } else if source.discs() < 8 {
            moves.into_iter().map(PlyAction::Play).collect()
        } else {
            Vec::new()
        };
        for action in actions {
            let goal = source.apply(action).unwrap();
            assert!(
                predecessors(goal.position()).contains(&(source.position(), action)),
                "missing independent forward edge: source={source:?}, action={action:?}"
            );
        }
    }
    for goal in targets_with_turn_changes(&reachable) {
        verify_all_predecessor_edges(goal);
    }
}

#[test]
fn predecessor_enumeration_preserves_forced_pass_and_terminal_turn() {
    // This independently checked fixture has the forced line H2, pass, H7.
    let actions = [PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)];
    let trace = middle_pass_start().replay(&actions).unwrap();
    for (index, &action) in actions.iter().enumerate() {
        let source = trace[index];
        let goal = trace[index + 1];
        assert!(predecessors(goal.position()).contains(&(source.position(), action)));
        verify_all_predecessor_edges(goal);
    }
    let terminal = *trace.last().unwrap();
    assert!(terminal.moves().is_empty());
    assert!(terminal.opposite_turn().moves().is_empty());
    assert!(terminal.apply(PlyAction::Pass).is_err());
    assert!(!predecessors(terminal.opposite_turn().position())
        .contains(&(terminal.position(), PlyAction::Pass)));
}
