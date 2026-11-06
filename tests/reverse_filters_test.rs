//! Reverse search is checked against the independent array-based rules oracle.
//! The expected classifications do not come from a previous search version.

mod support;

use std::collections::HashSet;

use othello_complexity_rs::othello::{Board, BoardSymmetry, Color, Position};
use othello_complexity_rs::query::candidates;
use othello_complexity_rs::record::{GoalMatch, PlyAction, PositionTransform, ReachabilityRecord};
use othello_complexity_rs::search::{
    engines::{
        bfs::retrospective_bfs,
        dfs::{retrospective_search, retrospective_search_move_ordering},
        parallel_dfs::retrospective_search_parallel,
        parallel_gbfs::parallel_retrospective_greedy_best_first_search,
    },
    forward::ForwardTable,
    moves::predecessors,
    outcome::SearchOutcome,
    transposition::Btable,
};

use support::{
    enumerate, middle_pass_start, random_game, targets_with_turn_changes, ArrayPosition,
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

fn equivalence_records(
    case: &str,
    goal: ArrayPosition,
    table: &ForwardTable,
) -> Vec<ReachabilityRecord> {
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
        pool.install(|| retrospective_bfs(goal.position(), table, usize::MAX, false))
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
                panic!("{case}: reachable equivalence query was rejected: {goal:?}")
            }
            SearchOutcome::Unknown { nodes } => {
                panic!(
                    "{case}: unbounded equivalence query returned UNKNOWN after {nodes} nodes: {goal:?}"
                )
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

fn bfs_with_threads(
    goal: Position,
    table: &ForwardTable,
    node_limit: usize,
    threads: usize,
) -> SearchOutcome {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| retrospective_bfs(goal, table, node_limit, false))
        .unwrap()
}

#[test]
fn unbounded_reverse_engines_match_across_thread_counts_and_goal_classes() {
    #[derive(Clone, Copy)]
    enum Engine {
        Gbfs,
        ParallelDfs,
        Bfs,
    }

    struct Scenario {
        name: &'static str,
        goal: ArrayPosition,
        boundary: u32,
        goal_match: GoalMatch,
        expected: bool,
        requires_pass: bool,
    }

    let fixed_actions = [
        PlyAction::Play(19),
        PlyAction::Play(20),
        PlyAction::Play(13),
        PlyAction::Play(12),
    ];
    let fixed_goal = *ArrayPosition::initial()
        .replay(&fixed_actions)
        .unwrap()
        .last()
        .unwrap();
    let (actions, trace) = reference_game(32);
    assert_eq!(actions[8], PlyAction::Pass);
    let pass_goal = trace[10];
    assert_eq!(pass_goal.discs(), 13);

    let reachable = enumerate(ArrayPosition::initial(), 7);
    let targets = targets_with_turn_changes(&reachable);
    let exact_unreachable = targets
        .iter()
        .copied()
        .find(|goal| goal.discs() == 7 && !reachable.contains(goal))
        .expect("the independent oracle includes a seven-disc unreachable query");
    let equivalence_unreachable = targets
        .into_iter()
        .find(|goal| goal.discs() == 7 && !equivalence_oracle_contains(&reachable, *goal))
        .expect("the independent oracle includes an unreachable equivalence query");

    let scenarios = [
        Scenario {
            name: "fixed-exact",
            goal: fixed_goal,
            boundary: 4,
            goal_match: GoalMatch::Exact,
            expected: true,
            requires_pass: false,
        },
        Scenario {
            name: "pass-exact",
            goal: pass_goal,
            boundary: 11,
            goal_match: GoalMatch::Exact,
            expected: true,
            requires_pass: true,
        },
        Scenario {
            name: "fixed-equivalence",
            goal: fixed_goal.transformed(BoardSymmetry::MirrorHorizontal),
            boundary: 4,
            goal_match: GoalMatch::EquivalenceClass,
            expected: true,
            requires_pass: false,
        },
        Scenario {
            name: "pass-equivalence",
            goal: pass_goal.transformed(BoardSymmetry::MirrorHorizontal),
            boundary: 11,
            goal_match: GoalMatch::EquivalenceClass,
            expected: true,
            requires_pass: true,
        },
        Scenario {
            name: "unreachable-exact",
            goal: exact_unreachable,
            boundary: 4,
            goal_match: GoalMatch::Exact,
            expected: false,
            requires_pass: false,
        },
        Scenario {
            name: "unreachable-equivalence",
            goal: equivalence_unreachable,
            boundary: 4,
            goal_match: GoalMatch::EquivalenceClass,
            expected: false,
            requires_pass: false,
        },
    ];

    for scenario in scenarios {
        let table = match scenario.goal_match {
            GoalMatch::Exact => {
                ForwardTable::new(scenario.boundary, Some(scenario.goal.position())).unwrap()
            }
            GoalMatch::EquivalenceClass => {
                ForwardTable::for_equivalence(scenario.boundary, scenario.goal.position()).unwrap()
            }
        };
        for engine in [Engine::Gbfs, Engine::ParallelDfs, Engine::Bfs] {
            let engine_name = match engine {
                Engine::Gbfs => "gbfs",
                Engine::ParallelDfs => "parallel-dfs",
                Engine::Bfs => "bfs",
            };
            let mut first_result = None;
            for threads in [1, 2, 8] {
                let outcome = match engine {
                    Engine::Gbfs => parallel_retrospective_greedy_best_first_search(
                        scenario.goal.position(),
                        &table,
                        usize::MAX,
                        false,
                        threads,
                    )
                    .unwrap(),
                    Engine::ParallelDfs => {
                        parallel_dfs_with_threads(scenario.goal.position(), &table, threads)
                    }
                    Engine::Bfs => {
                        bfs_with_threads(scenario.goal.position(), &table, usize::MAX, threads)
                    }
                };
                let row = format!("{}-{engine_name}-{threads}", scenario.name);
                assert!(
                    match &outcome {
                        SearchOutcome::Found { .. } => scenario.expected,
                        SearchOutcome::NotFound { .. } => !scenario.expected,
                        SearchOutcome::Unknown { .. } => false,
                    },
                    "{row}: outcome={outcome:?}"
                );
                if let Some(first_result) = first_result {
                    assert_eq!(outcome.result(), first_result, "{row}");
                } else {
                    first_result = Some(outcome.result());
                }
                if scenario.requires_pass && scenario.expected {
                    let SearchOutcome::Found { record, .. } = &outcome else {
                        panic!("{row}: expected a pass witness, got {outcome:?}");
                    };
                    assert!(record.actions.contains(&PlyAction::Pass), "{row}");
                }
                match scenario.goal_match {
                    GoalMatch::Exact => verify_outcome(scenario.goal, scenario.expected, outcome),
                    GoalMatch::EquivalenceClass => {
                        verify_equivalence_outcome(scenario.goal, scenario.expected, outcome)
                    }
                }
            }
        }
    }

    let reachable_eight = enumerate(ArrayPosition::initial(), 8);
    let exhausted = targets_with_turn_changes(&reachable_eight)
        .into_iter()
        .find(|goal| !reachable_eight.contains(goal))
        .expect("the eight-disc oracle includes an unreachable query");
    let table = ForwardTable::new(6, None).unwrap();
    for repeat in 0..8 {
        let outcome = parallel_retrospective_greedy_best_first_search(
            exhausted.position(),
            &table,
            usize::MAX,
            false,
            4,
        )
        .unwrap();
        assert!(
            matches!(&outcome, SearchOutcome::NotFound { .. }),
            "repeated-gbfs-4-{repeat}: outcome={outcome:?}"
        );
        verify_outcome(exhausted, false, outcome);
    }
}

#[test]
fn bfs_and_parallel_dfs_preserve_node_and_table_limit_boundaries() {
    #[derive(Clone, Copy)]
    enum Engine {
        Bfs,
        ParallelDfs { table_limit: usize },
    }

    enum Expected {
        Found {
            nodes: usize,
            actions: Vec<PlyAction>,
        },
        Unknown {
            nodes: usize,
        },
    }

    struct Case {
        name: &'static str,
        engine: Engine,
        goal: ArrayPosition,
        node_limit: usize,
        expected: Expected,
    }

    let initial = ArrayPosition::initial();
    let goal = initial.apply(PlyAction::Play(19)).unwrap();
    let table = ForwardTable::new(4, None).unwrap();
    let mut cases = vec![
        Case {
            name: "bfs-meeting-limit-0",
            engine: Engine::Bfs,
            goal: initial,
            node_limit: 0,
            expected: Expected::Found {
                nodes: 0,
                actions: Vec::new(),
            },
        },
        Case {
            name: "bfs-meeting-limit-1",
            engine: Engine::Bfs,
            goal: initial,
            node_limit: 1,
            expected: Expected::Found {
                nodes: 0,
                actions: Vec::new(),
            },
        },
        Case {
            name: "bfs-opening-limit-0",
            engine: Engine::Bfs,
            goal,
            node_limit: 0,
            expected: Expected::Unknown { nodes: 1 },
        },
        Case {
            name: "bfs-opening-limit-1",
            engine: Engine::Bfs,
            goal,
            node_limit: 1,
            expected: Expected::Found {
                nodes: 1,
                actions: vec![PlyAction::Play(19)],
            },
        },
    ];
    for table_limit in [0, 1, 2] {
        cases.push(Case {
            name: "parallel-dfs-before-expansion",
            engine: Engine::ParallelDfs { table_limit },
            goal,
            node_limit: 0,
            expected: Expected::Unknown { nodes: 1 },
        });
        cases.push(Case {
            name: "parallel-dfs-opening-found",
            engine: Engine::ParallelDfs { table_limit },
            goal,
            node_limit: 1,
            expected: Expected::Found {
                nodes: 1,
                actions: vec![PlyAction::Play(19)],
            },
        });
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    for case in cases {
        let table_limit = match case.engine {
            Engine::Bfs => None,
            Engine::ParallelDfs { table_limit } => Some(table_limit),
        };
        let outcome = match case.engine {
            Engine::Bfs => bfs_with_threads(case.goal.position(), &table, case.node_limit, 1),
            Engine::ParallelDfs { table_limit } => pool
                .install(|| {
                    retrospective_search_parallel(
                        case.goal.position(),
                        &table,
                        case.node_limit,
                        table_limit,
                    )
                })
                .unwrap(),
        };
        let row = format!("{}-table-{table_limit:?}", case.name);
        match case.expected {
            Expected::Found { nodes, actions } => {
                let SearchOutcome::Found {
                    record,
                    nodes: actual_nodes,
                } = outcome
                else {
                    panic!("{row}: expected FOUND, got {outcome:?}");
                };
                assert_eq!(actual_nodes, nodes, "{row}");
                assert_eq!(record.actions, actions, "{row}");
                assert_eq!(
                    record.verify().unwrap().last(),
                    Some(&case.goal.position()),
                    "{row}"
                );
            }
            Expected::Unknown { nodes } => assert!(
                matches!(outcome, SearchOutcome::Unknown { nodes: actual } if actual == nodes),
                "{row}: outcome={outcome:?}"
            ),
        }
    }
}

#[test]
fn bfs_deduplicates_before_reserving_the_node_limit() {
    #[derive(Clone, Copy)]
    enum Registration {
        Added,
        Dropped,
        Meeting,
    }

    fn required_squares(boundary: &[u64], position: Position) -> Option<u64> {
        let occupied = position.board.player | position.board.opponent;
        let mut matching = boundary
            .iter()
            .copied()
            .filter(|frontier| frontier & occupied == *frontier);
        let first = matching.next()?;
        Some(matching.fold(first, |common, frontier| common & frontier))
    }

    fn register(
        position: Position,
        table: &ForwardTable,
        boundary: &[u64],
        layer: &mut HashSet<Position>,
        registrations: &mut usize,
        attempts: &mut usize,
    ) -> Registration {
        if table.contains(position) {
            return Registration::Meeting;
        }
        if position.board.popcount() <= table.discs()
            || required_squares(boundary, position).is_none()
        {
            return Registration::Dropped;
        }
        *attempts += 1;
        if !layer.insert(position) {
            return Registration::Dropped;
        }
        *registrations += 1;
        Registration::Added
    }

    fn independent_exhaustion_count(
        goal: Position,
        table: &ForwardTable,
    ) -> Option<(usize, usize)> {
        let boundary: Vec<_> = table
            .positions()
            .filter(|position| position.board.popcount() == table.discs())
            .map(|position| position.board.player | position.board.opponent)
            .collect();
        let mut registrations = 0;
        let mut attempts = 0;
        let mut current = HashSet::new();
        if !matches!(
            register(
                goal,
                table,
                &boundary,
                &mut current,
                &mut registrations,
                &mut attempts,
            ),
            Registration::Added
        ) {
            return Some((registrations, attempts));
        }
        if let Some((before_pass, _)) = predecessors(goal)
            .into_iter()
            .find(|(_, action)| *action == PlyAction::Pass)
        {
            if matches!(
                register(
                    before_pass,
                    table,
                    &boundary,
                    &mut current,
                    &mut registrations,
                    &mut attempts,
                ),
                Registration::Meeting
            ) {
                return None;
            }
        }

        loop {
            let mut next = HashSet::new();
            for position in current {
                let required = required_squares(&boundary, position)
                    .expect("registered positions retain a compatible frontier");
                for (previous, action) in predecessors(position) {
                    let PlyAction::Play(square) = action else {
                        continue;
                    };
                    if required & (1_u64 << square) != 0 {
                        continue;
                    }
                    match register(
                        previous,
                        table,
                        &boundary,
                        &mut next,
                        &mut registrations,
                        &mut attempts,
                    ) {
                        Registration::Meeting => return None,
                        Registration::Dropped => continue,
                        Registration::Added => {}
                    }
                    if let Some((before_pass, _)) = predecessors(previous)
                        .into_iter()
                        .find(|(_, action)| *action == PlyAction::Pass)
                    {
                        if matches!(
                            register(
                                before_pass,
                                table,
                                &boundary,
                                &mut next,
                                &mut registrations,
                                &mut attempts,
                            ),
                            Registration::Meeting
                        ) {
                            return None;
                        }
                    }
                }
            }
            if next.is_empty() {
                return Some((registrations, attempts));
            }
            current = next;
        }
    }

    let start = ArrayPosition::initial().apply(PlyAction::Play(19)).unwrap();
    let reachable = enumerate(start, 9);
    let (goal, table, unique) = targets_with_turn_changes(&reachable)
        .into_iter()
        .filter(|goal| goal.discs() == 9 && !reachable.contains(goal))
        .find_map(|goal| {
            let table =
                ForwardTable::from_start(start.position(), 5, Some(goal.position())).ok()?;
            let (unique, attempts) = independent_exhaustion_count(goal.position(), &table)?;
            (unique >= 2 && attempts > unique).then_some((goal, table, unique))
        })
        .expect("the independent enumeration includes a duplicate-producing unreachable query");

    assert!(matches!(
        bfs_with_threads(goal.position(), &table, unique - 1, 1),
        SearchOutcome::Unknown { .. }
    ));
    assert!(matches!(
        bfs_with_threads(goal.position(), &table, unique, 1),
        SearchOutcome::NotFound { nodes } if nodes == unique
    ));
}

#[test]
fn bfs_exhaustion_counts_and_limits_are_stable_across_threads_and_goal_classes() {
    let reachable = enumerate(ArrayPosition::initial(), 8);
    let targets = targets_with_turn_changes(&reachable);
    for goal_match in [GoalMatch::Exact, GoalMatch::EquivalenceClass] {
        let (goal, table, expected_nodes) = targets
            .iter()
            .copied()
            .filter(|goal| {
                goal.discs() == 8
                    && match goal_match {
                        GoalMatch::Exact => !reachable.contains(goal),
                        GoalMatch::EquivalenceClass => {
                            !equivalence_oracle_contains(&reachable, *goal)
                        }
                    }
            })
            .find_map(|goal| {
                let table = match goal_match {
                    GoalMatch::Exact => ForwardTable::new(4, Some(goal.position())).unwrap(),
                    GoalMatch::EquivalenceClass => {
                        ForwardTable::for_equivalence(4, goal.position()).unwrap()
                    }
                };
                let outcome = bfs_with_threads(goal.position(), &table, usize::MAX, 1);
                match outcome {
                    SearchOutcome::NotFound { nodes } if nodes >= 2 => Some((goal, table, nodes)),
                    _ => None,
                }
            })
            .expect("the independent oracle includes a nontrivial unreachable query");

        for threads in [1, 2, 4] {
            let repeats = if threads == 1 { 1 } else { 4 };
            for repeat in 0..repeats {
                assert!(
                    matches!(
                        bfs_with_threads(goal.position(), &table, usize::MAX, threads),
                        SearchOutcome::NotFound { nodes } if nodes == expected_nodes
                    ),
                    "mode={goal_match}, threads={threads}, repeat={repeat}"
                );
                assert!(
                    matches!(
                        bfs_with_threads(goal.position(), &table, expected_nodes, threads),
                        SearchOutcome::NotFound { nodes } if nodes == expected_nodes
                    ),
                    "mode={goal_match}, threads={threads}, repeat={repeat}"
                );
                assert!(
                    matches!(
                        bfs_with_threads(goal.position(), &table, expected_nodes - 1, threads),
                        SearchOutcome::Unknown { .. }
                    ),
                    "mode={goal_match}, threads={threads}, repeat={repeat}"
                );
            }
        }
    }
}

#[test]
fn bfs_nodes_count_registered_positions_and_exclude_pruned_goals() {
    let initial = ArrayPosition::initial();
    let goal = initial.apply(PlyAction::Play(19)).unwrap();
    let table = ForwardTable::new(4, Some(goal.position())).unwrap();
    let SearchOutcome::Found { record, nodes } =
        bfs_with_threads(goal.position(), &table, usize::MAX, 1)
    else {
        panic!("the one-placement history must meet the opening table");
    };
    assert_eq!(nodes, 1);
    assert_eq!(record.actions, [PlyAction::Play(19)]);
    record.verify().unwrap();

    let mut pruned = Position::initial();
    pruned.board.player |= 1;
    let occupied = pruned.board.player | pruned.board.opponent;
    assert!(!othello_complexity_rs::filters::occupancy::check_occupancy(
        occupied
    ));
    let table = ForwardTable::new(4, Some(pruned)).unwrap();
    assert!(matches!(
        bfs_with_threads(pruned, &table, usize::MAX, 1),
        SearchOutcome::NotFound { nodes: 0 }
    ));
}

#[test]
fn bounded_bfs_never_reports_not_found_for_a_reachable_goal_near_its_limit() {
    let actions = [
        PlyAction::Play(19),
        PlyAction::Play(20),
        PlyAction::Play(13),
        PlyAction::Play(12),
    ];
    let goal = *ArrayPosition::initial()
        .replay(&actions)
        .unwrap()
        .last()
        .unwrap();
    let table = ForwardTable::new(4, Some(goal.position())).unwrap();

    let mut saw_bounded_found = false;
    for max_nodes in 1..=8 {
        for repeat in 0..12 {
            let outcome = bfs_with_threads(goal.position(), &table, max_nodes, 4);
            match outcome {
                SearchOutcome::Found { record, .. } => {
                    saw_bounded_found = true;
                    assert_eq!(
                        record.goal,
                        goal.position(),
                        "max_nodes={max_nodes}, repeat={repeat}"
                    );
                    record.verify().unwrap();
                }
                SearchOutcome::Unknown { .. } => {}
                SearchOutcome::NotFound { nodes } => {
                    panic!(
                        "reachable query was rejected after {nodes} nodes, max_nodes={max_nodes}, repeat={repeat}"
                    )
                }
            }
        }
    }
    assert!(saw_bounded_found, "bounded search never found a witness");

    let SearchOutcome::Found { record, .. } =
        bfs_with_threads(goal.position(), &table, usize::MAX, 4)
    else {
        panic!("unbounded search did not find the reachable goal");
    };
    assert_eq!(record.goal, goal.position());
    record.verify().unwrap();
}

#[test]
fn forward_table_matches_the_independent_eight_disc_oracle() {
    let expected = enumerate(ArrayPosition::initial(), 8);
    let table = ForwardTable::new(8, None).unwrap();
    let actual: HashSet<_> = table.positions().map(as_oracle).collect();

    assert_eq!(actual, expected);
    assert_eq!(table.len(), expected.len());
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
fn reverse_search_engines_match_independent_shallow_oracle_tables() {
    #[derive(Clone, Copy)]
    enum TargetSet {
        EightDiscs,
        SevenDiscs,
        RepresentativeTargeted,
    }

    #[derive(Clone, Copy)]
    enum Engine {
        Dfs,
        OrderedDfs,
        Gbfs { threads: usize },
        ParallelDfs { threads: usize, table_limit: usize },
        Bfs,
    }

    struct Row {
        name: &'static str,
        targets: TargetSet,
        boundary: u32,
        engine: Engine,
    }

    let reachable_eight = enumerate(ArrayPosition::initial(), 8);
    let targets_eight = targets_with_turn_changes(&reachable_eight);
    assert!(targets_eight
        .iter()
        .any(|goal| !reachable_eight.contains(goal)));
    assert!(targets_eight
        .iter()
        .any(|goal| reachable_eight.contains(goal)));

    let reachable_seven = enumerate(ArrayPosition::initial(), 7);
    let targets_seven = targets_with_turn_changes(&reachable_seven);
    assert!(targets_seven
        .iter()
        .any(|goal| !reachable_seven.contains(goal)));
    assert!(targets_seven
        .iter()
        .any(|goal| reachable_seven.contains(goal)));

    let mut bases: Vec<_> = reachable_eight
        .iter()
        .copied()
        .filter(|goal| goal.discs() == 8)
        .collect();
    bases.sort_by_key(|goal| goal.position());
    let mut representative_targets = Vec::new();
    for index in [0, bases.len() / 3, bases.len() * 2 / 3, bases.len() - 1] {
        let goal = bases[index];
        representative_targets.extend([
            goal,
            goal.opposite_turn(),
            goal.color_exchanged(),
            goal.transformed(BoardSymmetry::Rotate90),
        ]);
    }
    representative_targets.sort_by_key(|goal| goal.position());
    representative_targets.dedup();
    assert!(representative_targets
        .iter()
        .any(|goal| !reachable_eight.contains(goal)));

    let rows = [
        Row {
            name: "eight-boundary-6-dfs",
            targets: TargetSet::EightDiscs,
            boundary: 6,
            engine: Engine::Dfs,
        },
        Row {
            name: "eight-boundary-6-ordered-dfs",
            targets: TargetSet::EightDiscs,
            boundary: 6,
            engine: Engine::OrderedDfs,
        },
        Row {
            name: "eight-boundary-6-gbfs-1",
            targets: TargetSet::EightDiscs,
            boundary: 6,
            engine: Engine::Gbfs { threads: 1 },
        },
        Row {
            name: "eight-boundary-6-gbfs-3",
            targets: TargetSet::EightDiscs,
            boundary: 6,
            engine: Engine::Gbfs { threads: 3 },
        },
        Row {
            name: "eight-boundary-6-parallel-dfs",
            targets: TargetSet::EightDiscs,
            boundary: 6,
            engine: Engine::ParallelDfs {
                threads: 2,
                table_limit: 10_000,
            },
        },
        Row {
            name: "eight-boundary-6-bfs",
            targets: TargetSet::EightDiscs,
            boundary: 6,
            engine: Engine::Bfs,
        },
        Row {
            name: "seven-boundary-4-dfs",
            targets: TargetSet::SevenDiscs,
            boundary: 4,
            engine: Engine::Dfs,
        },
        Row {
            name: "seven-boundary-4-ordered-dfs",
            targets: TargetSet::SevenDiscs,
            boundary: 4,
            engine: Engine::OrderedDfs,
        },
        Row {
            name: "seven-boundary-4-parallel-dfs",
            targets: TargetSet::SevenDiscs,
            boundary: 4,
            engine: Engine::ParallelDfs {
                threads: 2,
                table_limit: 100_000,
            },
        },
        Row {
            name: "seven-boundary-4-gbfs-1",
            targets: TargetSet::SevenDiscs,
            boundary: 4,
            engine: Engine::Gbfs { threads: 1 },
        },
        Row {
            name: "eight-boundary-4-bfs",
            targets: TargetSet::EightDiscs,
            boundary: 4,
            engine: Engine::Bfs,
        },
        Row {
            name: "targeted-eight-boundary-6-dfs",
            targets: TargetSet::RepresentativeTargeted,
            boundary: 6,
            engine: Engine::Dfs,
        },
        Row {
            name: "targeted-eight-boundary-6-parallel-dfs",
            targets: TargetSet::RepresentativeTargeted,
            boundary: 6,
            engine: Engine::ParallelDfs {
                threads: 2,
                table_limit: 10_000,
            },
        },
        Row {
            name: "targeted-eight-boundary-6-gbfs-1",
            targets: TargetSet::RepresentativeTargeted,
            boundary: 6,
            engine: Engine::Gbfs { threads: 1 },
        },
        Row {
            name: "targeted-eight-boundary-6-gbfs-3",
            targets: TargetSet::RepresentativeTargeted,
            boundary: 6,
            engine: Engine::Gbfs { threads: 3 },
        },
    ];

    for row in rows {
        let (targets, reachable, targeted) = match row.targets {
            TargetSet::EightDiscs => (&targets_eight, &reachable_eight, false),
            TargetSet::SevenDiscs => (&targets_seven, &reachable_seven, false),
            TargetSet::RepresentativeTargeted => (&representative_targets, &reachable_eight, true),
        };
        let shared_table = (!targeted).then(|| ForwardTable::new(row.boundary, None).unwrap());
        let pool = match row.engine {
            Engine::ParallelDfs { threads, .. } => Some(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap(),
            ),
            _ => None,
        };
        let mut visited = Btable::new(10_000, 256);

        for &goal in targets {
            let expected = reachable.contains(&goal);
            let targeted_table =
                targeted.then(|| ForwardTable::new(row.boundary, Some(goal.position())).unwrap());
            let table = targeted_table.as_ref().or(shared_table.as_ref()).unwrap();
            let outcome = match row.engine {
                Engine::Dfs => {
                    visited.clear();
                    retrospective_search(goal.position(), table, &mut visited, usize::MAX).unwrap()
                }
                Engine::OrderedDfs => {
                    visited.clear();
                    retrospective_search_move_ordering(
                        goal.position(),
                        table,
                        &mut visited,
                        usize::MAX,
                    )
                    .unwrap()
                }
                Engine::Gbfs { threads } => parallel_retrospective_greedy_best_first_search(
                    goal.position(),
                    table,
                    usize::MAX,
                    false,
                    threads,
                )
                .unwrap(),
                Engine::ParallelDfs { table_limit, .. } => pool
                    .as_ref()
                    .unwrap()
                    .install(|| {
                        retrospective_search_parallel(
                            goal.position(),
                            table,
                            usize::MAX,
                            table_limit,
                        )
                    })
                    .unwrap(),
                Engine::Bfs => {
                    retrospective_bfs(goal.position(), table, usize::MAX, false).unwrap()
                }
            };
            assert!(
                match &outcome {
                    SearchOutcome::Found { .. } => expected,
                    SearchOutcome::NotFound { .. } => !expected,
                    SearchOutcome::Unknown { .. } => false,
                },
                "{}: goal={goal:?}, outcome={outcome:?}",
                row.name
            );
            verify_outcome(goal, expected, outcome);
        }
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
                pool.install(|| retrospective_bfs(goal.position(), &table, usize::MAX, false))
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
                        retrospective_bfs(candidate, &candidate_table, usize::MAX, false)
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
            let outcome = retrospective_bfs(goal, &table, budget, false).unwrap();
            if matches!(outcome, SearchOutcome::Unknown { .. }) {
                saw_frontier = true;
            } else {
                verify_outcome(array_goal, true, outcome);
            }
        }
    }
    assert!(saw_frontier, "some mid-game goals must exhaust the budget");
}

#[test]
fn equivalence_searches_preserve_forward_reverse_pass_and_terminal_histories() {
    #[derive(Clone, Copy)]
    enum Check {
        ForwardPass,
        ReversePass,
        PassParity(usize),
        NoPass,
    }

    struct Row {
        name: &'static str,
        goal: ArrayPosition,
        boundary: u32,
        check: Check,
    }

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

    assert!(actions[..8].iter().all(|action| *action != PlyAction::Pass));
    let (terminal_actions, terminal_trace) = reference_game(1194);
    let terminal = *terminal_trace.last().unwrap();
    assert_eq!(terminal_actions.len(), 10);
    assert_eq!(terminal.discs(), 14);
    assert!(terminal_actions
        .iter()
        .all(|action| *action != PlyAction::Pass));
    assert!(terminal.moves().is_empty());
    assert!(terminal.opposite_turn().moves().is_empty());
    assert!(terminal.apply(PlyAction::Pass).is_err());

    let rows = [
        Row {
            name: "forward-pass",
            goal,
            boundary: 12,
            check: Check::ForwardPass,
        },
        Row {
            name: "reverse-pass",
            goal,
            boundary: 11,
            check: Check::ReversePass,
        },
        Row {
            name: "even-pass-history",
            goal: trace[8],
            boundary: 11,
            check: Check::PassParity(0),
        },
        Row {
            name: "odd-pass-history",
            goal: trace[9],
            boundary: 11,
            check: Check::PassParity(1),
        },
        Row {
            name: "early-terminal",
            goal: terminal,
            boundary: 10,
            check: Check::NoPass,
        },
    ];

    for row in rows {
        let table = ForwardTable::for_equivalence(row.boundary, row.goal.position()).unwrap();
        match row.check {
            Check::ForwardPass => {
                assert!(table.contains(after_pass.position()), "{}", row.name);
                let (prefix, _) = table
                    .actions_to(after_pass.position())
                    .expect("the forward side of the meeting must retain the pass");
                assert!(prefix.contains(&PlyAction::Pass), "{}", row.name);
                for record in equivalence_records(row.name, row.goal, &table) {
                    assert!(record.actions.contains(&PlyAction::Pass), "{}", row.name);
                }
            }
            Check::ReversePass => {
                assert!(!table.contains(before_pass.position()), "{}", row.name);
                assert!(!table.contains(after_pass.position()), "{}", row.name);
                for record in equivalence_records(row.name, row.goal, &table) {
                    let pass = record
                        .actions
                        .iter()
                        .position(|action| *action == PlyAction::Pass)
                        .expect("the reverse side of the meeting must retain the pass");
                    let placements_before_pass = record.actions[..pass]
                        .iter()
                        .filter(|action| matches!(action, PlyAction::Play(_)))
                        .count();
                    assert_eq!(placements_before_pass, 8, "{}", row.name);
                }
            }
            Check::PassParity(expected_parity) => {
                for record in equivalence_records(row.name, row.goal, &table) {
                    let passes = record
                        .actions
                        .iter()
                        .filter(|action| **action == PlyAction::Pass)
                        .count();
                    assert_eq!(passes % 2, expected_parity, "{}", row.name);
                }
            }
            Check::NoPass => {
                for record in equivalence_records(row.name, row.goal, &table) {
                    assert!(
                        record
                            .actions
                            .iter()
                            .all(|action| *action != PlyAction::Pass),
                        "{}",
                        row.name
                    );
                }
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
