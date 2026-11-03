//! Reverse search is checked against the independent array-based rules oracle.
//! The expected classifications do not come from a previous search version.

mod support;

use std::collections::HashSet;

use othello_complexity_rs::filters::{
    impossible_edges::check_virtual_edge_patterns, occupancy::check_occupancy,
    seg3::check_seg3_more,
};
use othello_complexity_rs::othello::{Board, BoardSymmetry, Color, Position, CENTER_MASK};
use othello_complexity_rs::query::candidates;
use othello_complexity_rs::record::{GoalMatch, PlyAction, PositionTransform, ReachabilityRecord};
use othello_complexity_rs::search::{
    bfs::{retrospective_bfs, BfsOutcome},
    core::{predecessors, successors, SearchOutcome},
    dfs::retrospective_search,
    forward::ForwardTable,
    inmemory_bfs::parallel_inmemory_retrospective_bfs,
    move_ordering::retrospective_search_move_ordering,
    parallel_dfs::retrospective_search_parallel,
    parallel_gbfs::parallel_retrospective_greedy_best_first_search,
    transposition::Btable,
};

use rand::{Rng, SeedableRng};
use support::{
    enumerate, middle_pass_start, random_game, targets_with_turn_changes, ArrayPosition,
    ALL_SYMMETRIES,
};

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

fn verify_equivalence_outcome(goal: ArrayPosition, expected: bool, outcome: SearchOutcome) {
    let actual = equivalence_outcome_is_reachable(goal, outcome);
    assert_eq!(
        actual, expected,
        "equivalence classification differs from the oracle: {goal:?}"
    );
}

fn equivalence_outcome_is_reachable(goal: ArrayPosition, outcome: SearchOutcome) -> bool {
    match outcome {
        SearchOutcome::Found { record, .. } => {
            verify_equivalence_record(goal, &record);
            true
        }
        SearchOutcome::NotFound { .. } => false,
        SearchOutcome::Unknown { nodes } => {
            panic!("unbounded equivalence query returned UNKNOWN after {nodes} nodes: {goal:?}");
        }
    }
}

fn verify_equivalence_record(goal: ArrayPosition, record: &ReachabilityRecord) {
    assert_eq!(record.start, Position::initial());
    assert_eq!(record.goal, goal.position());
    assert_eq!(record.goal_match, GoalMatch::EquivalenceClass);
    let independent_trace = ArrayPosition::initial()
        .replay(&record.actions)
        .expect("the equivalence certificate must obey the independent rules");
    assert_eq!(
        independent_trace.last().unwrap().position(),
        record.transform.apply_position(goal.position())
    );
    record.verify().unwrap();
}

fn equivalence_records(goal: ArrayPosition, table: &ForwardTable) -> Vec<ReachabilityRecord> {
    let mut visited = Btable::new(100_000, 256);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let outcomes = [
        retrospective_search(goal.position(), table, &mut visited, usize::MAX).unwrap(),
        parallel_retrospective_greedy_best_first_search(
            goal.position(),
            table,
            usize::MAX,
            false,
            1,
        )
        .unwrap(),
        pool.install(|| {
            parallel_inmemory_retrospective_bfs(goal.position(), table, usize::MAX, false, false)
        })
        .unwrap(),
    ];
    outcomes
        .into_iter()
        .map(|outcome| match outcome {
            SearchOutcome::Found { record, .. } => {
                verify_equivalence_record(goal, &record);
                record
            }
            SearchOutcome::NotFound { .. } => {
                panic!("reachable equivalence query was rejected: {goal:?}")
            }
            SearchOutcome::Unknown { nodes } => {
                panic!("unbounded equivalence query returned UNKNOWN after {nodes} nodes: {goal:?}")
            }
        })
        .collect()
}

fn reference_game(seed: u64) -> (Vec<PlyAction>, Vec<ArrayPosition>) {
    let actions: Vec<_> = random_game(seed)
        .into_iter()
        .map(|(_, action)| action)
        .collect();
    let trace = ArrayPosition::initial()
        .replay(&actions)
        .expect("the fixed-seed reference game must obey the independent rules");
    (actions, trace)
}

fn outcome_is_reachable(goal: Position, outcome: SearchOutcome) -> bool {
    match outcome {
        SearchOutcome::Found { record, .. } => {
            assert_eq!(record.goal, goal);
            assert_eq!(record.goal_match, GoalMatch::Exact);
            record.verify().unwrap();
            true
        }
        SearchOutcome::NotFound { .. } => false,
        SearchOutcome::Unknown { nodes } => {
            panic!("unbounded exact candidate returned UNKNOWN after {nodes} nodes: {goal:?}")
        }
    }
}

fn equivalence_oracle_contains(reachable: &HashSet<ArrayPosition>, goal: ArrayPosition) -> bool {
    goal.orbit()
        .flat_map(|image| [image, image.color_exchanged()])
        .any(|image| reachable.contains(&image))
}

fn parallel_dfs_with_threads(
    goal: Position,
    table: &ForwardTable,
    threads: usize,
) -> SearchOutcome {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| retrospective_search_parallel(goal, table, usize::MAX, usize::MAX))
        .unwrap()
}

fn inmemory_bfs_with_threads(
    goal: Position,
    table: &ForwardTable,
    node_limit: usize,
    threads: usize,
) -> SearchOutcome {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| parallel_inmemory_retrospective_bfs(goal, table, node_limit, false, false))
        .unwrap()
}

#[test]
fn inmemory_bfs_reconstructs_pass_terminal_and_equivalence_witnesses() {
    let (actions, trace) = reference_game(32);
    assert_eq!(actions[8], PlyAction::Pass);
    let before_pass = trace[8];
    let after_pass = trace[9];
    assert_eq!(before_pass.apply(PlyAction::Pass).unwrap(), after_pass);

    let exact_table = ForwardTable::new(11, Some(after_pass.position())).unwrap();
    assert!(!exact_table.contains(before_pass.position()));
    assert!(!exact_table.contains(after_pass.position()));
    let outcome = inmemory_bfs_with_threads(after_pass.position(), &exact_table, usize::MAX, 1);
    let SearchOutcome::Found { record, .. } = outcome else {
        panic!("outcome={outcome:?}");
    };
    assert!(record.actions.contains(&PlyAction::Pass));
    assert_eq!(record.goal_match, GoalMatch::Exact);
    assert_eq!(
        ArrayPosition::initial()
            .replay(&record.actions)
            .unwrap()
            .last(),
        Some(&after_pass)
    );
    record.verify().unwrap();

    let equivalence_goal = trace[10].transformed(BoardSymmetry::MirrorHorizontal);
    let equivalence_table = ForwardTable::for_equivalence(11, equivalence_goal.position()).unwrap();
    let outcome = inmemory_bfs_with_threads(
        equivalence_goal.position(),
        &equivalence_table,
        usize::MAX,
        3,
    );
    let SearchOutcome::Found { record, .. } = outcome else {
        panic!("outcome={outcome:?}");
    };
    assert!(record.actions.contains(&PlyAction::Pass));
    verify_equivalence_record(equivalence_goal, &record);

    let (_, terminal_trace) = reference_game(1194);
    let terminal = *terminal_trace.last().unwrap();
    assert!(terminal.moves().is_empty());
    assert!(terminal.opposite_turn().moves().is_empty());
    let terminal_table = ForwardTable::for_equivalence(10, terminal.position()).unwrap();
    let outcome = inmemory_bfs_with_threads(terminal.position(), &terminal_table, usize::MAX, 3);
    let SearchOutcome::Found { record, .. } = outcome else {
        panic!("outcome={outcome:?}");
    };
    verify_equivalence_record(terminal, &record);
}

#[test]
fn inmemory_bfs_checks_meetings_before_zero_and_one_node_limits() {
    let initial = ArrayPosition::initial();
    let table = ForwardTable::new(4, None).unwrap();
    for node_limit in [0, 1] {
        let outcome = inmemory_bfs_with_threads(initial.position(), &table, node_limit, 1);
        let SearchOutcome::Found { record, nodes: 0 } = outcome else {
            panic!("node_limit={node_limit}, outcome={outcome:?}");
        };
        assert!(record.actions.is_empty());
        record.verify().unwrap();
    }

    let goal = initial.apply(PlyAction::Play(19)).unwrap();
    assert!(matches!(
        inmemory_bfs_with_threads(goal.position(), &table, 0, 1),
        SearchOutcome::Unknown { nodes: 0 }
    ));
    let outcome = inmemory_bfs_with_threads(goal.position(), &table, 1, 1);
    let SearchOutcome::Found { record, nodes: 1 } = outcome else {
        panic!("outcome={outcome:?}");
    };
    assert_eq!(record.actions, [PlyAction::Play(19)]);
    assert_eq!(record.verify().unwrap().last(), Some(&goal.position()));
}

#[test]
fn inmemory_bfs_deduplicates_before_reserving_the_node_limit() {
    let standard = ArrayPosition::initial();
    let start = standard.apply(PlyAction::Play(19)).unwrap();
    let actions = [
        PlyAction::Play(20),
        PlyAction::Play(13),
        PlyAction::Play(12),
        PlyAction::Play(37),
    ];
    let goal = *start.replay(&actions).unwrap().last().unwrap();
    let table = ForwardTable::from_start(start.position(), 5, Some(goal.position())).unwrap();

    assert!(matches!(
        inmemory_bfs_with_threads(goal.position(), &table, 14, 1),
        SearchOutcome::Unknown { .. }
    ));
    let outcome = inmemory_bfs_with_threads(goal.position(), &table, 15, 1);
    let SearchOutcome::Found { record, .. } = outcome else {
        panic!("outcome={outcome:?}");
    };
    assert_eq!(record.start, start.position());
    assert_eq!(record.goal, goal.position());
    assert_eq!(record.verify().unwrap().last(), Some(&goal.position()));
}

#[test]
fn inmemory_bfs_skips_standard_pruning_for_a_custom_start() {
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
    let table = ForwardTable::from_start(start, 5, Some(goal)).unwrap();
    let outcome = inmemory_bfs_with_threads(goal, &table, usize::MAX, 1);
    let SearchOutcome::Found { record, nodes: 1 } = outcome else {
        panic!("outcome={outcome:?}");
    };
    assert_eq!(record.start, start);
    assert_eq!(record.goal, goal);
    assert_eq!(record.actions, [action]);
    assert_eq!(record.verify().unwrap().last(), Some(&goal));
}

#[test]
fn unbounded_inmemory_bfs_results_match_across_thread_counts() {
    let exact_actions = [
        PlyAction::Play(19),
        PlyAction::Play(20),
        PlyAction::Play(13),
        PlyAction::Play(12),
    ];
    let exact_goal = *ArrayPosition::initial()
        .replay(&exact_actions)
        .unwrap()
        .last()
        .unwrap();
    let exact_table = ForwardTable::new(4, Some(exact_goal.position())).unwrap();
    let exact = [1, 4].map(|threads| {
        inmemory_bfs_with_threads(exact_goal.position(), &exact_table, usize::MAX, threads)
    });
    assert_eq!(exact[0].result(), exact[1].result());
    for outcome in exact {
        verify_outcome(exact_goal, true, outcome);
    }

    let equivalence_goal = exact_goal.transformed(BoardSymmetry::MirrorHorizontal);
    let equivalence_table = ForwardTable::for_equivalence(4, equivalence_goal.position()).unwrap();
    let equivalence = [1, 4].map(|threads| {
        inmemory_bfs_with_threads(
            equivalence_goal.position(),
            &equivalence_table,
            usize::MAX,
            threads,
        )
    });
    assert_eq!(equivalence[0].result(), equivalence[1].result());
    for outcome in equivalence {
        verify_equivalence_outcome(equivalence_goal, true, outcome);
    }

    let reachable = enumerate(ArrayPosition::initial(), 7);
    let unreachable = targets_with_turn_changes(&reachable)
        .into_iter()
        .find(|goal| goal.discs() == 7 && !reachable.contains(goal))
        .expect("the independent oracle includes a seven-disc unreachable query");
    let unreachable_table = ForwardTable::new(4, Some(unreachable.position())).unwrap();
    let exhausted = [1, 4].map(|threads| {
        inmemory_bfs_with_threads(
            unreachable.position(),
            &unreachable_table,
            usize::MAX,
            threads,
        )
    });
    assert_eq!(exhausted[0].result(), exhausted[1].result());
    for outcome in exhausted {
        verify_outcome(unreachable, false, outcome);
    }
}

#[test]
fn unbounded_gbfs_results_match_across_thread_counts() {
    let (actions, trace) = reference_game(32);
    assert_eq!(actions[8], PlyAction::Pass);
    let exact_goal = trace[10];
    let exact_table = ForwardTable::new(11, Some(exact_goal.position())).unwrap();
    let exact = [1, 2, 8].map(|threads| {
        parallel_retrospective_greedy_best_first_search(
            exact_goal.position(),
            &exact_table,
            usize::MAX,
            false,
            threads,
        )
        .unwrap()
    });
    let exact_result = exact[0].result();
    for (threads, outcome) in [1, 2, 8].into_iter().zip(exact) {
        assert_eq!(outcome.result(), exact_result, "threads={threads}");
        let SearchOutcome::Found { record, .. } = &outcome else {
            panic!("threads={threads}, outcome={outcome:?}");
        };
        assert!(record.actions.contains(&PlyAction::Pass));
        record.verify().unwrap();
        verify_outcome(exact_goal, true, outcome);
    }

    let equivalence_goal = exact_goal.transformed(BoardSymmetry::MirrorHorizontal);
    let equivalence_table = ForwardTable::for_equivalence(11, equivalence_goal.position()).unwrap();
    let equivalence = [1, 2, 8].map(|threads| {
        parallel_retrospective_greedy_best_first_search(
            equivalence_goal.position(),
            &equivalence_table,
            usize::MAX,
            false,
            threads,
        )
        .unwrap()
    });
    let equivalence_result = equivalence[0].result();
    for (threads, outcome) in [1, 2, 8].into_iter().zip(equivalence) {
        assert_eq!(outcome.result(), equivalence_result, "threads={threads}");
        let SearchOutcome::Found { record, .. } = &outcome else {
            panic!("threads={threads}, outcome={outcome:?}");
        };
        assert!(record.actions.contains(&PlyAction::Pass));
        record.verify().unwrap();
        verify_equivalence_outcome(equivalence_goal, true, outcome);
    }

    let reachable = enumerate(ArrayPosition::initial(), 7);
    let targets = targets_with_turn_changes(&reachable);
    let exact_unreachable = targets
        .iter()
        .copied()
        .find(|goal| goal.discs() == 7 && !reachable.contains(goal))
        .expect("the independent oracle includes a seven-disc unreachable query");
    let exact_table = ForwardTable::new(4, Some(exact_unreachable.position())).unwrap();
    let exact = [1, 2, 8].map(|threads| {
        parallel_retrospective_greedy_best_first_search(
            exact_unreachable.position(),
            &exact_table,
            usize::MAX,
            false,
            threads,
        )
        .unwrap()
    });
    let exact_result = exact[0].result();
    for (threads, outcome) in [1, 2, 8].into_iter().zip(exact) {
        assert_eq!(outcome.result(), exact_result, "threads={threads}");
        verify_outcome(exact_unreachable, false, outcome);
    }

    let equivalence_unreachable = targets
        .into_iter()
        .find(|goal| goal.discs() == 7 && !equivalence_oracle_contains(&reachable, *goal))
        .expect("the independent oracle includes an unreachable equivalence query");
    let equivalence_table =
        ForwardTable::for_equivalence(4, equivalence_unreachable.position()).unwrap();
    let equivalence = [1, 2, 8].map(|threads| {
        parallel_retrospective_greedy_best_first_search(
            equivalence_unreachable.position(),
            &equivalence_table,
            usize::MAX,
            false,
            threads,
        )
        .unwrap()
    });
    let equivalence_result = equivalence[0].result();
    for (threads, outcome) in [1, 2, 8].into_iter().zip(equivalence) {
        assert_eq!(outcome.result(), equivalence_result, "threads={threads}");
        verify_equivalence_outcome(equivalence_unreachable, false, outcome);
    }
}

#[test]
fn forward_table_matches_the_independent_eight_disc_oracle() {
    let expected = enumerate(ArrayPosition::initial(), 8);
    let table = ForwardTable::new(8, None).unwrap();
    let actual: HashSet<_> = table.positions().map(as_oracle).collect();

    assert_eq!(actual, expected);
    for position in table.positions() {
        let (actions, symmetry) = table
            .actions_to(position)
            .expect("every forward position must have a witness");
        assert_eq!(symmetry, BoardSymmetry::Identity);
        let trace = ArrayPosition::initial().replay(&actions).unwrap();
        assert_eq!(trace.last(), Some(&as_oracle(position)));
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
    for &goal in &targets {
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
        for threads in [1, 3] {
            verify_outcome(
                goal,
                expected,
                parallel_retrospective_greedy_best_first_search(
                    goal.position(),
                    &table,
                    usize::MAX,
                    false,
                    threads,
                )
                .unwrap(),
            );
        }
    }

    let exhausted = *targets
        .iter()
        .find(|goal| !reachable.contains(goal))
        .expect("the fixture includes an unreachable query");
    for _ in 0..8 {
        verify_outcome(
            exhausted,
            false,
            parallel_retrospective_greedy_best_first_search(
                exhausted.position(),
                &table,
                usize::MAX,
                false,
                4,
            )
            .unwrap(),
        );
    }
}

#[test]
fn unbounded_bfs_matches_all_eight_disc_oracle_queries() {
    let reachable = enumerate(ArrayPosition::initial(), 8);
    let targets = targets_with_turn_changes(&reachable);
    for discs in [4, 6] {
        let table = ForwardTable::new(discs, None).unwrap();
        for &goal in &targets {
            let expected = reachable.contains(&goal);
            match retrospective_bfs(goal.position(), &table, usize::MAX).unwrap() {
                BfsOutcome::Resolved(outcome) => verify_outcome(goal, expected, outcome),
                BfsOutcome::Frontier { .. } => {
                    panic!("an unbounded reverse BFS must not stop early: {goal:?}")
                }
            }
        }
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
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
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
            pool.install(|| {
                retrospective_search_parallel(goal.position(), &table, usize::MAX, 100_000)
            })
            .unwrap(),
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
fn parallel_dfs_preserves_small_node_and_table_limit_boundaries() {
    let goal = ArrayPosition::initial()
        .apply(PlyAction::Play(19))
        .expect("D3 is a legal opening move");
    let table = ForwardTable::new(4, None).unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();

    for table_limit in [0, 1, 2] {
        let outcome = pool
            .install(|| retrospective_search_parallel(goal.position(), &table, 0, table_limit))
            .unwrap();
        assert!(
            matches!(outcome, SearchOutcome::Unknown { nodes: 1 }),
            "table_limit={table_limit}, outcome={outcome:?}"
        );

        let outcome = pool
            .install(|| retrospective_search_parallel(goal.position(), &table, 1, table_limit))
            .unwrap();
        let SearchOutcome::Found { record, nodes: 1 } = outcome else {
            panic!("table_limit={table_limit}, outcome={outcome:?}");
        };
        assert_eq!(record.actions, [PlyAction::Play(19)]);
        assert_eq!(record.verify().unwrap().last(), Some(&goal.position()));
    }
}

#[test]
fn one_thread_parallel_dfs_matches_sequential_exhaustion_counts() {
    let reachable = enumerate(ArrayPosition::initial(), 7);
    let table = ForwardTable::new(4, None).unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut visited = Btable::new(usize::MAX, 256);
    let mut saw_nontrivial_search = false;

    for goal in targets_with_turn_changes(&reachable)
        .into_iter()
        .filter(|goal| goal.discs() == 7 && !reachable.contains(goal))
    {
        visited.clear();
        let sequential =
            retrospective_search_move_ordering(goal.position(), &table, &mut visited, usize::MAX)
                .unwrap();
        let parallel = pool
            .install(|| {
                retrospective_search_parallel(goal.position(), &table, usize::MAX, usize::MAX)
            })
            .unwrap();
        assert!(matches!(sequential, SearchOutcome::NotFound { .. }));
        assert!(matches!(parallel, SearchOutcome::NotFound { .. }));
        assert_eq!(parallel.nodes(), sequential.nodes(), "goal={goal:?}");
        saw_nontrivial_search |= sequential.nodes() > 1;
    }
    assert!(saw_nontrivial_search);
}

#[test]
fn parallel_dfs_unbounded_results_match_across_thread_counts() {
    let (actions, trace) = reference_game(32);
    assert_eq!(actions[8], PlyAction::Pass);
    let exact_goal = trace[10];
    let exact_table = ForwardTable::new(11, Some(exact_goal.position())).unwrap();

    for threads in [1, 2, 8] {
        let outcome = parallel_dfs_with_threads(exact_goal.position(), &exact_table, threads);
        let SearchOutcome::Found { record, .. } = outcome else {
            panic!("threads={threads}, outcome={outcome:?}");
        };
        assert_eq!(record.goal_match, GoalMatch::Exact);
        assert!(record.actions.contains(&PlyAction::Pass));
        let independent_trace = ArrayPosition::initial().replay(&record.actions).unwrap();
        assert_eq!(independent_trace.last(), Some(&exact_goal));
        record.verify().unwrap();
    }

    let equivalence_goal = exact_goal.transformed(BoardSymmetry::MirrorHorizontal);
    let equivalence_table = ForwardTable::for_equivalence(11, equivalence_goal.position()).unwrap();
    for threads in [1, 2, 8] {
        let outcome =
            parallel_dfs_with_threads(equivalence_goal.position(), &equivalence_table, threads);
        let SearchOutcome::Found { record, .. } = outcome else {
            panic!("threads={threads}, outcome={outcome:?}");
        };
        assert!(record.actions.contains(&PlyAction::Pass));
        verify_equivalence_record(equivalence_goal, &record);
    }

    let reachable = enumerate(ArrayPosition::initial(), 7);
    let unreachable = targets_with_turn_changes(&reachable)
        .into_iter()
        .find(|goal| goal.discs() == 7 && !reachable.contains(goal))
        .expect("the independent oracle includes a seven-disc unreachable query");
    let exhausted_table = ForwardTable::new(4, None).unwrap();
    for threads in [1, 2, 8] {
        let outcome = parallel_dfs_with_threads(unreachable.position(), &exhausted_table, threads);
        assert!(
            matches!(outcome, SearchOutcome::NotFound { .. }),
            "threads={threads}, outcome={outcome:?}"
        );
    }
}

#[test]
fn one_equivalence_search_matches_all_exact_candidates_through_eight_discs() {
    let reachable = enumerate(ArrayPosition::initial(), 8);
    let targets = targets_with_turn_changes(&reachable);
    let mirrored_start = ArrayPosition::initial().transformed(BoardSymmetry::MirrorHorizontal);
    let exchanged_start = ArrayPosition::initial().color_exchanged();
    assert!(!reachable.contains(&mirrored_start));
    assert!(!reachable.contains(&exchanged_start));
    assert!(targets.contains(&mirrored_start));
    assert!(targets.contains(&exchanged_start));
    assert!(targets
        .iter()
        .any(|&goal| !equivalence_oracle_contains(&reachable, goal)));

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let mut visited = Btable::new(10_000, 256);
    for discs in 4..=7 {
        for &goal in &targets {
            let expected = equivalence_oracle_contains(&reachable, goal);
            let table = ForwardTable::for_equivalence(discs, goal.position()).unwrap();

            visited.clear();
            verify_equivalence_outcome(
                goal,
                expected,
                retrospective_search(goal.position(), &table, &mut visited, usize::MAX).unwrap(),
            );
            verify_equivalence_outcome(
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
            verify_equivalence_outcome(
                goal,
                expected,
                pool.install(|| {
                    parallel_inmemory_retrospective_bfs(
                        goal.position(),
                        &table,
                        usize::MAX,
                        false,
                        false,
                    )
                })
                .unwrap(),
            );

            let mut exact_reachable = [false; 3];
            for (_, candidate) in candidates(
                Position::initial(),
                goal.position(),
                GoalMatch::EquivalenceClass,
            ) {
                let candidate_expected = reachable.contains(&as_oracle(candidate));
                let candidate_table = ForwardTable::new(discs, Some(candidate)).unwrap();

                visited.clear();
                let found = outcome_is_reachable(
                    candidate,
                    retrospective_search(candidate, &candidate_table, &mut visited, usize::MAX)
                        .unwrap(),
                );
                assert_eq!(found, candidate_expected);
                exact_reachable[0] |= found;

                let found = outcome_is_reachable(
                    candidate,
                    parallel_retrospective_greedy_best_first_search(
                        candidate,
                        &candidate_table,
                        usize::MAX,
                        false,
                        1,
                    )
                    .unwrap(),
                );
                assert_eq!(found, candidate_expected);
                exact_reachable[1] |= found;

                let found = outcome_is_reachable(
                    candidate,
                    pool.install(|| {
                        parallel_inmemory_retrospective_bfs(
                            candidate,
                            &candidate_table,
                            usize::MAX,
                            false,
                            false,
                        )
                    })
                    .unwrap(),
                );
                assert_eq!(found, candidate_expected);
                exact_reachable[2] |= found;
            }
            assert_eq!(
                exact_reachable, [expected; 3],
                "goal={goal:?}, discs={discs}"
            );
        }
    }
}

#[test]
fn equivalence_dfs_and_gbfs_are_invariant_under_all_board_symmetries() {
    let reachable = enumerate(ArrayPosition::initial(), 8);
    let mut reachable_goals: Vec<_> = reachable
        .iter()
        .copied()
        .filter(|goal| goal.discs() == 8)
        .collect();
    reachable_goals.sort_by_key(|goal| goal.position());
    let reachable_goal = reachable_goals[reachable_goals.len() / 2];
    let unreachable_goal = targets_with_turn_changes(&reachable)
        .into_iter()
        .find(|goal| goal.discs() == 8 && !equivalence_oracle_contains(&reachable, *goal))
        .expect("the independent oracle includes an unreachable eight-disc query");
    let mut visited = Btable::new(10_000, 256);

    for goal in [reachable_goal, unreachable_goal] {
        let expected = equivalence_oracle_contains(&reachable, goal);
        for symmetry in ALL_SYMMETRIES {
            let image = goal.transformed(symmetry);
            assert_eq!(equivalence_oracle_contains(&reachable, image), expected);
            let table = ForwardTable::for_equivalence(6, image.position()).unwrap();

            visited.clear();
            let dfs = equivalence_outcome_is_reachable(
                image,
                retrospective_search(image.position(), &table, &mut visited, usize::MAX).unwrap(),
            );
            let gbfs = equivalence_outcome_is_reachable(
                image,
                parallel_retrospective_greedy_best_first_search(
                    image.position(),
                    &table,
                    usize::MAX,
                    false,
                    1,
                )
                .unwrap(),
            );
            assert_eq!(dfs, expected, "goal={goal:?}, symmetry={symmetry:?}");
            assert_eq!(gbfs, dfs, "goal={goal:?}, symmetry={symmetry:?}");
        }
    }
}

#[test]
fn equivalence_table_contains_exactly_the_orbits_reachable_through_eight_discs() {
    let reachable = enumerate(ArrayPosition::initial(), 8);
    let targets = targets_with_turn_changes(&reachable);
    let mut saw_reachable = false;
    let mut saw_unreachable = false;

    for goal in targets {
        let expected = equivalence_oracle_contains(&reachable, goal);
        saw_reachable |= expected;
        saw_unreachable |= !expected;
        let table = ForwardTable::for_equivalence(goal.discs() as u32, goal.position()).unwrap();

        for stored in table.positions() {
            let [player, opponent] = stored.board.unique();
            assert_eq!(stored.board, Board::new(player, opponent));
        }
        for image in goal.orbit() {
            assert_eq!(
                table.contains(image.position()),
                expected,
                "goal={goal:?}, image={image:?}"
            );
        }
    }

    assert!(saw_reachable);
    assert!(saw_unreachable);
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
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
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
                pool.install(|| {
                    retrospective_search_parallel(goal.position(), table, usize::MAX, 10_000)
                })
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

#[test]
fn bounded_bfs_never_refutes_a_legal_history() {
    let table = ForwardTable::new(8, None).unwrap();
    let mut saw_frontier = false;
    for seed in 0..24 {
        let history = random_game(seed);
        // Mid-game goals keep the reverse search from exhausting quickly.
        let prefix = 20 + (seed as usize * 5) % 25;
        let prefix = prefix.min(history.len() - 1);
        let (before_last, last) = history[prefix];
        let goal = othello_complexity_rs::record::replay(before_last, &[last]).unwrap()[1];
        let array_goal = ArrayPosition::from_relative(
            &goal.board.to_string(),
            goal.side_to_move == Color::Black,
        );
        for budget in [200, 5_000] {
            match retrospective_bfs(goal, &table, budget).unwrap() {
                BfsOutcome::Resolved(outcome) => verify_outcome(array_goal, true, outcome),
                BfsOutcome::Frontier { .. } => saw_frontier = true,
            }
        }
    }
    assert!(saw_frontier, "some mid-game goals must exhaust the budget");
}

#[test]
fn equivalence_searches_reconstruct_passes_on_both_sides_of_the_meeting() {
    let (actions, trace) = reference_game(32);
    assert_eq!(actions[8], PlyAction::Pass);
    assert_eq!(
        actions
            .iter()
            .filter(|action| **action == PlyAction::Pass)
            .count(),
        1,
        "the fixed-seed game must exercise a legal pass"
    );
    let before_pass = trace[8];
    let after_pass = trace[9];
    let goal = trace[10];
    assert!(before_pass.moves().is_empty());
    assert!(!after_pass.moves().is_empty());
    assert_eq!(before_pass.apply(PlyAction::Pass).unwrap(), after_pass);

    let forward_pass_table = ForwardTable::for_equivalence(12, goal.position()).unwrap();
    assert!(forward_pass_table.contains(after_pass.position()));
    let (prefix, _) = forward_pass_table
        .actions_to(after_pass.position())
        .expect("the forward side of the meeting must retain the pass");
    assert!(prefix.contains(&PlyAction::Pass));
    for record in equivalence_records(goal, &forward_pass_table) {
        assert!(record.actions.contains(&PlyAction::Pass));
    }

    let reverse_pass_table = ForwardTable::for_equivalence(11, goal.position()).unwrap();
    assert!(!reverse_pass_table.contains(before_pass.position()));
    assert!(!reverse_pass_table.contains(after_pass.position()));
    for record in equivalence_records(goal, &reverse_pass_table) {
        let pass = record
            .actions
            .iter()
            .position(|action| *action == PlyAction::Pass)
            .expect("the reverse side of the meeting must retain the pass");
        let placements_before_pass = record.actions[..pass]
            .iter()
            .filter(|action| matches!(action, PlyAction::Play(_)))
            .count();
        assert_eq!(placements_before_pass, 8);
    }
}

#[test]
fn equivalence_searches_preserve_even_and_odd_pass_histories() {
    let (actions, trace) = reference_game(32);
    assert!(actions[..8].iter().all(|action| *action != PlyAction::Pass));
    assert_eq!(actions[8], PlyAction::Pass);

    for (goal, expected_parity) in [(trace[8], 0), (trace[9], 1)] {
        let table = ForwardTable::for_equivalence(11, goal.position()).unwrap();
        for record in equivalence_records(goal, &table) {
            let passes = record
                .actions
                .iter()
                .filter(|action| **action == PlyAction::Pass)
                .count();
            assert_eq!(passes % 2, expected_parity);
        }
    }
}

#[test]
fn equivalence_search_maps_a_witness_from_the_mirrored_opening() {
    let mirrored_start = ArrayPosition::initial().transformed(BoardSymmetry::MirrorHorizontal);
    let goal = mirrored_start.apply(PlyAction::Play(20)).unwrap();
    let reachable = enumerate(ArrayPosition::initial(), 5);
    assert!(!reachable.contains(&goal));

    let table = ForwardTable::for_equivalence(4, goal.position()).unwrap();
    let (actions, symmetry) = table.actions_to(mirrored_start.position()).unwrap();
    assert!(actions.is_empty());
    assert_eq!(symmetry, BoardSymmetry::MirrorHorizontal);
    for record in equivalence_records(goal, &table) {
        assert_eq!(record.actions, [PlyAction::Play(19)]);
        assert_eq!(
            record.transform,
            PositionTransform {
                symmetry: BoardSymmetry::MirrorHorizontal,
                exchange_colors: false,
            }
        );
    }
}

#[test]
fn equivalence_searches_accept_an_early_terminal_position() {
    let (actions, trace) = reference_game(1194);
    let terminal = *trace.last().unwrap();
    assert_eq!(actions.len(), 10);
    assert_eq!(terminal.discs(), 14);
    assert!(actions.iter().all(|action| *action != PlyAction::Pass));
    assert!(terminal.moves().is_empty());
    assert!(terminal.opposite_turn().moves().is_empty());
    assert!(terminal.apply(PlyAction::Pass).is_err());

    let table = ForwardTable::for_equivalence(10, terminal.position()).unwrap();
    for record in equivalence_records(terminal, &table) {
        assert!(record
            .actions
            .iter()
            .all(|action| *action != PlyAction::Pass));
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

#[test]
fn reverse_pruning_conditions_are_invariant_under_all_board_symmetries() {
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x8a11_5eed);
    for _ in 0..1024 {
        let occupied = rng.random::<u64>() | CENTER_MASK;
        let player = rng.random::<u64>() & occupied;
        let board = Board::new(player, occupied ^ player);
        let expected_occupancy = check_occupancy(occupied);
        let expected_edges = check_virtual_edge_patterns(board.player, board.opponent);
        let expected_seg3 = check_seg3_more(board.player, board.opponent, false);

        for symmetry in ALL_SYMMETRIES {
            let image = symmetry.apply(board);
            assert_eq!(
                check_occupancy(image.player | image.opponent),
                expected_occupancy,
                "occupancy symmetry={symmetry:?}, board={board:?}"
            );
            assert_eq!(
                check_virtual_edge_patterns(image.player, image.opponent),
                expected_edges,
                "virtual edges symmetry={symmetry:?}, board={board:?}"
            );
            assert_eq!(
                check_seg3_more(image.player, image.opponent, false),
                expected_seg3,
                "seg3 symmetry={symmetry:?}, board={board:?}"
            );
        }
    }
}
