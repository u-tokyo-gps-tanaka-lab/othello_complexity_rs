//! The SAT driver is checked against the independent array-based rules oracle.
//! The nopass-witness encoding can prove reachability, but not unreachability.

mod support;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use othello_complexity_rs::filters::sat::{
    run_with_options, validate_options, ReverseBfs, RunOptions, SatEncoding, SatPortfolio,
};
use othello_complexity_rs::othello::{Board, BoardSymmetry, Position};
use othello_complexity_rs::query::{candidates, GoalClassification};
use othello_complexity_rs::record::{
    format_actions, parse_actions, GoalMatch, PlyAction, PositionTransform, ReachabilityRecord,
};

use support::{
    enumerate, middle_pass_start, query_targets, random_game, targets_with_turn_changes,
    ArrayPosition, ALL_SYMMETRIES,
};

fn options(
    start: Position,
    goals: Vec<Position>,
    goal_match: GoalMatch,
    encodings: impl Into<SatPortfolio>,
) -> RunOptions {
    RunOptions {
        start,
        goals,
        goal_match,
        encodings: encodings.into(),
        seeds: 1,
        parallel_goals: 4,
        show_coords: false,
        show_boards: false,
        verbose: false,
        cnf_dump_dir: None,
        cnf_dump_only: false,
        sat_timeout_per_instance: None,
        reverse_bfs: None,
    }
}

fn validation_options() -> RunOptions {
    options(
        Position::initial(),
        Vec::new(),
        GoalMatch::Exact,
        SatEncoding::PlacementRays,
    )
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "sat-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Debug)]
struct Dimacs {
    variables: usize,
    clauses: Vec<Vec<i32>>,
}

fn parse_dimacs(text: &str) -> Dimacs {
    let mut lines = text.lines();
    let header = lines.next().expect("DIMACS header");
    let fields: Vec<_> = header.split_whitespace().collect();
    assert_eq!(fields.len(), 4);
    assert_eq!(&fields[..2], ["p", "cnf"]);
    let variables = fields[2].parse::<usize>().unwrap();
    let clause_count = fields[3].parse::<usize>().unwrap();
    let clauses = lines
        .map(|line| {
            let mut literals: Vec<_> = line
                .split_whitespace()
                .map(|field| field.parse::<i32>().unwrap())
                .collect();
            assert_eq!(literals.pop(), Some(0));
            assert!(!literals.contains(&0));
            assert!(literals
                .iter()
                .all(|literal| literal.unsigned_abs() as usize <= variables));
            literals
        })
        .collect::<Vec<_>>();
    assert_eq!(clauses.len(), clause_count);
    Dimacs { variables, clauses }
}

fn dump_exact_cnf(
    encoding: SatEncoding,
    start: Position,
    goal: Position,
) -> (GoalClassification, Dimacs) {
    let directory = TestDirectory::new();
    let run_options = RunOptions {
        cnf_dump_dir: Some(directory.path().to_path_buf()),
        ..options(start, vec![goal], GoalMatch::Exact, encoding)
    };
    let mut callbacks = 0;
    let outcomes = run_with_options(run_options, |_| {
        callbacks += 1;
        Ok(())
    })
    .unwrap_or_else(|error| panic!("{encoding}: {error}"));
    assert_eq!(callbacks, 1, "{encoding}");
    let [outcome]: [GoalClassification; 1] = outcomes.try_into().unwrap();
    let text = fs::read_to_string(
        directory
            .path()
            .join(format!("goal_0001_identity_{encoding}.cnf")),
    )
    .unwrap();
    (outcome, parse_dimacs(&text))
}

/// Every SAT answer must carry a witness that independently replays at every
/// layer and ends at the recorded image of the original query.
fn verify_answer(
    start: ArrayPosition,
    goal: ArrayPosition,
    goal_match: GoalMatch,
    input_index: usize,
    outcome: &GoalClassification,
    expected_reachable: bool,
) {
    assert_eq!(outcome.goal(), goal.position());
    assert_eq!(outcome.input_index(), input_index);
    assert_eq!(outcome.goal_match(), goal_match);
    match outcome {
        GoalClassification::Reachable { record } => {
            assert!(expected_reachable, "unexpected SAT: {record:?}");
            assert_eq!(record.start, start.position());
            assert_eq!(record.goal, goal.position());
            assert_eq!(record.goal_match, goal_match);
            assert_eq!(record.input_index, input_index);
            if goal_match == GoalMatch::Exact {
                assert_eq!(record.transform, PositionTransform::IDENTITY);
            }
            if goal_match != GoalMatch::EquivalenceClass {
                assert!(!record.transform.exchange_colors);
            }
            let independent_trace = start.replay(&record.actions).unwrap();
            let endpoint = *independent_trace.last().unwrap();
            let image = goal.transformed(record.transform.symmetry);
            let expected_endpoint = if record.transform.exchange_colors {
                image.color_exchanged()
            } else {
                image
            };
            assert_eq!(endpoint, expected_endpoint);
            let independent_trace: Vec<_> = independent_trace
                .into_iter()
                .map(ArrayPosition::position)
                .collect();
            assert_eq!(record.verify().unwrap(), independent_trace);
            assert!(outcome.record().is_some());
        }
        GoalClassification::Unreachable { .. } => {
            assert!(
                !expected_reachable,
                "unexpected UNSAT: start={:?}, goal={:?}, match={goal_match:?}",
                start.position(),
                goal.position(),
            );
            assert!(outcome.record().is_none());
        }
        GoalClassification::Unknown { .. } => panic!("unbounded small query returned UNKNOWN"),
    }
}

fn classify(
    start: ArrayPosition,
    goals: &[ArrayPosition],
    goal_match: GoalMatch,
    expected: impl Fn(ArrayPosition) -> bool,
    expected_actions: Option<&[PlyAction]>,
) -> Vec<GoalClassification> {
    let positions: Vec<_> = goals.iter().copied().map(ArrayPosition::position).collect();
    let mut first = None;
    for encoding in SatEncoding::ALL
        .into_iter()
        .filter(|encoding| encoding.proves_unreachable())
    {
        let mut callbacks = Vec::new();
        let outcomes = run_with_options(
            options(start.position(), positions.clone(), goal_match, encoding),
            |result| {
                callbacks.push(result.input_index());
                Ok(())
            },
        )
        .unwrap_or_else(|error| panic!("{encoding}: {error}"));
        assert_eq!(outcomes.len(), goals.len(), "{encoding}");
        callbacks.sort_unstable();
        assert_eq!(
            callbacks,
            (1..=goals.len()).collect::<Vec<_>>(),
            "{encoding}"
        );
        for (index, outcome) in outcomes.iter().enumerate() {
            let input_index = outcome.input_index();
            assert_eq!(input_index, index + 1, "{encoding}");
            let goal = goals[input_index - 1];
            verify_answer(
                start,
                goal,
                goal_match,
                input_index,
                outcome,
                expected(goal),
            );
            if let Some(expected_actions) = expected_actions {
                let record = outcome
                    .record()
                    .expect("expected actions require a reachable outcome");
                assert_eq!(record.actions.as_slice(), expected_actions, "{encoding}");
            }
        }
        if first.is_none() {
            first = Some(outcomes);
        }
    }
    first.expect("SatEncoding::ALL is not empty")
}

fn exact_record(start: ArrayPosition, actions: &[PlyAction]) -> ReachabilityRecord {
    let trace = start.replay(actions).unwrap();
    ReachabilityRecord {
        input_index: 1,
        start: start.position(),
        goal: trace.last().unwrap().position(),
        goal_match: GoalMatch::Exact,
        transform: PositionTransform::IDENTITY,
        actions: actions.to_vec(),
    }
}

fn enumerate_placements(start: ArrayPosition, max_discs: usize) -> HashSet<ArrayPosition> {
    let mut seen = HashSet::new();
    let mut pending = vec![start];
    while let Some(position) = pending.pop() {
        if !seen.insert(position) || position.discs() >= max_discs {
            continue;
        }
        for square in position.moves() {
            pending.push(position.apply(PlyAction::Play(square)).unwrap());
        }
    }
    seen
}

fn has_witness(
    reachable: &HashSet<ArrayPosition>,
    goal: ArrayPosition,
    goal_match: GoalMatch,
) -> bool {
    match goal_match {
        GoalMatch::Exact => reachable.contains(&goal),
        GoalMatch::SymmetryOrbit => goal.orbit().any(|image| reachable.contains(&image)),
        GoalMatch::EquivalenceClass => goal.orbit().any(|image| {
            reachable.contains(&image) || reachable.contains(&image.color_exchanged())
        }),
    }
}

fn verify_witness(
    start: ArrayPosition,
    goal: ArrayPosition,
    goal_match: GoalMatch,
    outcome: &GoalClassification,
) {
    let record = outcome
        .record()
        .expect("a reachable answer carries a record");
    assert_eq!(record.start, start.position());
    assert!(record
        .actions
        .iter()
        .all(|action| matches!(action, PlyAction::Play(_))));
    if goal_match == GoalMatch::Exact {
        assert_eq!(record.transform, PositionTransform::IDENTITY);
    }
    if goal_match != GoalMatch::EquivalenceClass {
        assert!(!record.transform.exchange_colors);
    }
    let mut endpoint = goal.transformed(record.transform.symmetry);
    if record.transform.exchange_colors {
        endpoint = endpoint.color_exchanged();
    }
    let trace = start.replay(&record.actions).unwrap();
    assert_eq!(trace.last(), Some(&endpoint));
    assert_eq!(record.actions.len(), endpoint.discs() - start.discs());
    assert_eq!(
        record.verify().unwrap(),
        trace
            .into_iter()
            .map(ArrayPosition::position)
            .collect::<Vec<_>>()
    );
}

fn check_queries(start: ArrayPosition, goals: &[ArrayPosition], goal_match: GoalMatch) {
    let max_discs = goals.iter().map(|goal| goal.discs()).max().unwrap();
    let reachable = enumerate_placements(start, max_discs);
    let run_options = RunOptions {
        parallel_goals: 3,
        ..options(
            start.position(),
            goals.iter().map(|goal| goal.position()).collect(),
            goal_match,
            SatEncoding::NoPassWitness,
        )
    };
    let mut callbacks = Vec::new();
    let outcomes = run_with_options(run_options, |outcome| {
        callbacks.push(outcome.input_index());
        Ok(())
    })
    .unwrap();
    assert_eq!(outcomes.len(), goals.len());
    callbacks.sort_unstable();
    assert_eq!(callbacks, (1..=goals.len()).collect::<Vec<_>>());
    for (index, (goal, outcome)) in goals.iter().copied().zip(&outcomes).enumerate() {
        assert_eq!(outcome.input_index(), index + 1);
        assert_eq!(outcome.goal(), goal.position());
        assert_eq!(outcome.goal_match(), goal_match);
        let expected = has_witness(&reachable, goal, goal_match);
        match outcome {
            GoalClassification::Reachable { .. } => {
                assert!(expected, "unexpected witness: {start:?} -> {goal:?}");
                verify_witness(start, goal, goal_match, outcome);
            }
            GoalClassification::Unknown { .. } => {
                assert!(
                    !expected,
                    "missed an unbounded small witness: {start:?} -> {goal:?}"
                );
            }
            GoalClassification::Unreachable { .. } => {
                panic!("no-pass failure must remain UNKNOWN: {start:?} -> {goal:?}");
            }
        }
    }
}

#[test]
fn independent_initial_rules_and_geometric_transforms_match_the_contract() {
    let initial = ArrayPosition::initial();
    assert_eq!(initial.position(), Position::initial());
    // D3, C4, F5, E6 are the four ordinary Othello opening placements.
    assert_eq!(initial.moves(), vec![19, 26, 37, 44]);
    assert_eq!(BoardSymmetry::ALL.len(), 8);
    for source in [initial, middle_pass_start()] {
        for symmetry in ALL_SYMMETRIES {
            assert!(BoardSymmetry::ALL.contains(&symmetry));
            assert_eq!(
                symmetry.apply_position(source.position()),
                source.transformed(symmetry).position()
            );
        }
    }
}

/// Running the reverse BFS inside the SAT driver must not change any answer for
/// either complete placement encoding.
#[test]
fn bfs_driver_agrees_with_plain_sat_on_six_disc_queries() {
    let start = ArrayPosition::initial();
    let reachable = enumerate(start, 6);
    let goals = targets_with_turn_changes(&reachable);
    assert!(goals.iter().any(|goal| !reachable.contains(goal)));
    classify(
        start,
        &goals,
        GoalMatch::Exact,
        |goal| reachable.contains(&goal),
        None,
    );

    let goal_positions = goals.iter().map(|goal| goal.position()).collect::<Vec<_>>();
    for encodings in [
        SatPortfolio::from(SatEncoding::PlacementColumns),
        SatPortfolio::from(SatEncoding::PlacementRays),
    ] {
        for bfs in [
            Some(ReverseBfs {
                discs: 4,
                state_budget: 3,
            }),
            Some(ReverseBfs {
                discs: 5,
                state_budget: usize::MAX,
            }),
        ] {
            let outcomes = run_with_options(
                RunOptions {
                    parallel_goals: 2,
                    reverse_bfs: bfs,
                    ..options(
                        start.position(),
                        goal_positions.clone(),
                        GoalMatch::Exact,
                        encodings.clone(),
                    )
                },
                |_| Ok(()),
            )
            .unwrap();
            assert_eq!(outcomes.len(), goals.len());
            for (goal, outcome) in goals.iter().zip(&outcomes) {
                let expected = reachable.contains(goal);
                match outcome {
                    GoalClassification::Reachable { record } => {
                        assert!(expected, "{encodings} {bfs:?}: {goal:?}");
                        assert_eq!(record.verify().unwrap().last(), Some(&goal.position()));
                    }
                    GoalClassification::Unreachable { .. } => {
                        assert!(!expected, "{encodings} {bfs:?}: {goal:?}");
                    }
                    GoalClassification::Unknown { .. } => {
                        panic!("{encodings} {bfs:?}: unknown for {goal:?}")
                    }
                }
            }
        }
    }
}

#[test]
fn all_seven_disc_targets_and_both_turns_agree_with_independent_enumeration() {
    let start = ArrayPosition::initial();
    let reachable = enumerate(start, 7);
    let goals = query_targets(&reachable);
    assert!(goals.iter().any(|goal| !reachable.contains(goal)));
    for goal_match in [
        GoalMatch::Exact,
        GoalMatch::SymmetryOrbit,
        GoalMatch::EquivalenceClass,
    ] {
        classify(
            start,
            &goals,
            goal_match,
            |goal| match goal_match {
                GoalMatch::Exact => reachable.contains(&goal),
                GoalMatch::SymmetryOrbit => goal.orbit().any(|image| reachable.contains(&image)),
                GoalMatch::EquivalenceClass => goal.orbit().any(|image| {
                    reachable.contains(&image) || reachable.contains(&image.color_exchanged())
                }),
            },
            None,
        );
    }
}

#[test]
fn representative_seven_and_eight_disc_queries_match_independent_enumeration() {
    let start = ArrayPosition::initial();
    let reachable = enumerate(start, 8);
    let mut goals = Vec::new();
    for discs in [7, 8] {
        let mut bases: Vec<_> = reachable
            .iter()
            .copied()
            .filter(|goal| goal.discs() == discs)
            .collect();
        bases.sort_by_key(|goal| goal.position());
        for index in [0, bases.len() / 3, bases.len() * 2 / 3, bases.len() - 1] {
            let goal = bases[index];
            goals.extend([
                goal,
                goal.opposite_turn(),
                goal.color_exchanged(),
                goal.transformed(BoardSymmetry::Rotate90),
            ]);
        }
    }
    goals.sort_by_key(|goal| goal.position());
    goals.dedup();
    assert!(goals.iter().any(|goal| reachable.contains(goal)));
    assert!(goals.iter().any(|goal| !reachable.contains(goal)));
    classify(
        start,
        &goals,
        GoalMatch::Exact,
        |goal| reachable.contains(&goal),
        None,
    );
}

#[test]
fn leading_middle_trailing_passes_and_terminal_turns_are_preserved() {
    let fixture = middle_pass_start();
    for start in [fixture, fixture.color_exchanged()] {
        let actions = [PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)];
        let trace = start.replay(&actions).unwrap();
        let first = trace[1];
        let after_pass = trace[2];
        let terminal = trace[3];
        assert_eq!(start.moves(), vec![15]);
        assert!(first.moves().is_empty());
        assert_eq!(after_pass.moves(), vec![55]);
        assert!(terminal.moves().is_empty());
        assert!(terminal.apply(PlyAction::Pass).is_err());
        for (source, goals) in [
            (start, vec![start, first, after_pass, terminal]),
            (first, vec![first, after_pass, terminal]),
            (after_pass, vec![after_pass, terminal]),
            (terminal, vec![terminal, terminal.opposite_turn(), start]),
        ] {
            check_queries(source, &goals, GoalMatch::Exact);
        }
        for (source, goal, expected) in [
            (start, terminal, &actions[..]),
            (first, terminal, &actions[1..]),
            (start, after_pass, &actions[..2]),
            (first, after_pass, &actions[1..2]),
            (after_pass, terminal, &actions[2..]),
        ] {
            classify(source, &[goal], GoalMatch::Exact, |_| true, Some(expected));
        }
        let reachable = enumerate(start, 64);
        assert!(reachable.contains(&terminal));
        let wrong_terminal_turn = terminal.opposite_turn();
        assert!(!reachable.contains(&wrong_terminal_turn));
        classify(
            start,
            &[wrong_terminal_turn],
            GoalMatch::Exact,
            |_| false,
            None,
        );
        classify(terminal, &[terminal], GoalMatch::Exact, |_| true, Some(&[]));
        classify(
            terminal,
            &[wrong_terminal_turn],
            GoalMatch::Exact,
            |_| false,
            None,
        );
    }
}

#[test]
fn zero_placement_queries_allow_identity_or_one_legal_pass_only() {
    let start = ArrayPosition::initial();
    let reflected = start.transformed(BoardSymmetry::MirrorHorizontal);
    assert_ne!(start, reflected);
    assert_eq!(reflected.color_exchanged(), start.opposite_turn());
    assert!(start.apply(PlyAction::Pass).is_err());
    let goals = [
        start,
        start.color_exchanged(),
        reflected,
        reflected.color_exchanged(),
    ];
    let exact = classify(start, &goals, GoalMatch::Exact, |goal| goal == start, None);
    let symmetric = classify(
        start,
        &goals,
        GoalMatch::SymmetryOrbit,
        |goal| goal == start || goal == reflected,
        None,
    );
    let equivalent = classify(start, &goals, GoalMatch::EquivalenceClass, |_| true, None);
    for outcome in exact.iter().chain(&symmetric).chain(&equivalent) {
        if let Some(record) = outcome.record() {
            assert!(record.actions.is_empty());
        }
    }
    let record_of = |input_index| {
        equivalent
            .iter()
            .find(|outcome| outcome.input_index() == input_index)
            .unwrap()
            .record()
            .unwrap()
    };
    // Zero-length witnesses keep Black to move, so the turn fixes the exchange flag.
    assert!(record_of(2).transform.exchange_colors);
    let reflected_record = record_of(3);
    assert_ne!(reflected_record.transform.symmetry, BoardSymmetry::Identity);
    assert!(!reflected_record.transform.exchange_colors);

    let fixture = ArrayPosition::from_relative(
        "OXX-OOOOOXXXXXXOOXOOOXXOOOXOXOXXOXOXOOXXOOOXXXOXOOXOOOXXOOOOOOXX",
        false,
    );
    for start in [fixture, fixture.color_exchanged()] {
        let after_pass = start.apply(PlyAction::Pass).unwrap();
        assert_eq!(after_pass.moves(), vec![3]);
        assert_eq!(
            enumerate(start, start.discs()),
            HashSet::from([start, after_pass])
        );
        classify(start, &[start], GoalMatch::Exact, |_| true, Some(&[]));
        classify(
            start,
            &[after_pass],
            GoalMatch::Exact,
            |_| true,
            Some(&[PlyAction::Pass]),
        );
        assert!(after_pass.apply(PlyAction::Pass).is_err());
        classify(after_pass, &[start], GoalMatch::Exact, |_| false, None);
    }

    for terminal in [
        ArrayPosition::from_relative(&"X".repeat(64), true),
        ArrayPosition::from_relative(
            "---------------------------XX------XX---------------------------",
            true,
        ),
    ] {
        assert!(terminal.moves().is_empty());
        assert!(terminal.opposite_turn().moves().is_empty());
        assert!(terminal.apply(PlyAction::Pass).is_err());
        classify(terminal, &[terminal], GoalMatch::Exact, |_| true, Some(&[]));
        classify(
            terminal,
            &[terminal.opposite_turn()],
            GoalMatch::Exact,
            |_| false,
            None,
        );
    }
}

#[test]
fn a_single_placement_can_require_both_leading_and_trailing_passes() {
    let fixture = ArrayPosition::from_relative(
        "-XOOOOOO-XOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOO",
        false,
    );
    let actions = [PlyAction::Pass, PlyAction::Play(8), PlyAction::Pass];
    for start in [fixture, fixture.color_exchanged()] {
        let trace = start.replay(&actions).unwrap();
        assert!(start.moves().is_empty());
        assert_eq!(trace[1].moves(), vec![0, 8]);
        assert!(trace[2].moves().is_empty());
        let goal = trace[3];
        assert_eq!(goal.moves(), vec![0]);
        assert_eq!(goal.discs(), start.discs() + 1);
        classify(start, &[goal], GoalMatch::Exact, |_| true, Some(&actions));
        // Two mandatory passes preserve the placement-only turn parity. The
        // absence of a legal first placement, not parity, excludes this witness.
        check_queries(start, &[goal], GoalMatch::Exact);

        let portfolio = "nopass-witness+placement-columns"
            .parse::<SatPortfolio>()
            .unwrap();
        let outcomes = run_with_options(
            RunOptions {
                parallel_goals: 1,
                ..options(
                    start.position(),
                    vec![goal.position()],
                    GoalMatch::Exact,
                    portfolio,
                )
            },
            |_| Ok(()),
        )
        .unwrap_or_else(|error| {
            panic!("portfolio failed for start={start:?}, goal={goal:?}: {error}")
        });
        let [outcome]: [GoalClassification; 1] = outcomes.try_into().unwrap();
        match outcome {
            GoalClassification::Reachable { record } => {
                assert_eq!(record.actions.as_slice(), &actions);
                let trace = start.replay(&record.actions).unwrap();
                assert_eq!(trace.last(), Some(&goal));
                assert_eq!(
                    record.verify().unwrap(),
                    trace
                        .into_iter()
                        .map(ArrayPosition::position)
                        .collect::<Vec<_>>()
                );
            }
            GoalClassification::Unreachable { .. } => {
                panic!("portfolio rejected a reachable goal")
            }
            GoalClassification::Unknown { .. } => {
                panic!("witness-only UNSAT stopped the complete encoding")
            }
        }
    }
}

#[test]
fn target_mask_must_preserve_legal_moves_outside_the_goal_when_checking_passes() {
    let start = ArrayPosition::initial();
    let reachable = enumerate(start, 5);

    // E3 is legal for White from the initial colors. Black's four legal first
    // moves all lie outside this goal's occupancy, but Black must still move:
    // filtering the legal-move predicate by the target would invent a pass.
    let false_leading_pass_goal = start.opposite_turn().apply(PlyAction::Play(20)).unwrap();
    let board = false_leading_pass_goal.position().board;
    let occupied = board.player | board.opponent;
    assert!(!start.moves().is_empty());
    assert!(start
        .moves()
        .iter()
        .all(|&square| occupied & (1_u64 << square) == 0));
    assert!(start.apply(PlyAction::Pass).is_err());
    assert!(!reachable.contains(&false_leading_pass_goal));
    classify(
        start,
        &[false_leading_pass_goal],
        GoalMatch::Exact,
        |_| false,
        None,
    );

    // After the ordinary D3 opening, White also has legal moves outside the
    // five-disc goal. A target mask must not permit a false trailing pass.
    let after_opening = start.apply(PlyAction::Play(19)).unwrap();
    let false_trailing_pass_goal = after_opening.opposite_turn();
    let board = false_trailing_pass_goal.position().board;
    let occupied = board.player | board.opponent;
    assert!(!after_opening.moves().is_empty());
    assert!(after_opening
        .moves()
        .iter()
        .all(|&square| occupied & (1_u64 << square) == 0));
    assert!(after_opening.apply(PlyAction::Pass).is_err());
    assert!(!reachable.contains(&false_trailing_pass_goal));
    classify(
        start,
        &[false_trailing_pass_goal],
        GoalMatch::Exact,
        |_| false,
        None,
    );
}

#[test]
fn stone_count_cannot_decrease_and_opening_move_changes_turn() {
    let start = ArrayPosition::initial();
    let after_move = start.apply(PlyAction::Play(19)).unwrap();
    let goals = [after_move, after_move.color_exchanged()];
    classify(
        start,
        &goals,
        GoalMatch::Exact,
        |goal| goal == after_move,
        None,
    );
    let equivalent = classify(start, &goals, GoalMatch::EquivalenceClass, |_| true, None);
    // Every five-disc position has White to move, so the Black-to-move board
    // is reached only through the absolute color exchange.
    let exchanged = equivalent
        .iter()
        .find(|outcome| outcome.input_index() == 2)
        .unwrap()
        .record()
        .unwrap();
    assert!(exchanged.transform.exchange_colors);
    // The opening move always leaves both colors, so no image of this
    // monochromatic board is reachable; the turn parity still admits one or
    // two plies, so the query reaches the SAT encoding.
    let monochromatic = ArrayPosition::from_relative(
        "-------------------X-------XX------XX---------------------------",
        true,
    );
    for goal_match in [
        GoalMatch::Exact,
        GoalMatch::SymmetryOrbit,
        GoalMatch::EquivalenceClass,
    ] {
        classify(after_move, &[start], goal_match, |_| false, None);
        classify(start, &[monochromatic], goal_match, |_| false, None);
    }
}

#[test]
fn simultaneous_geometry_and_absolute_color_exchange_preserve_reachability() {
    let start = middle_pass_start();
    let terminal = *start
        .replay(&[PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)])
        .unwrap()
        .last()
        .unwrap();
    for symmetry in ALL_SYMMETRIES {
        let source = start.transformed(symmetry);
        let target = terminal.transformed(symmetry);
        for (source, target) in [
            (source, target),
            (source.color_exchanged(), target.color_exchanged()),
        ] {
            let reachable = enumerate(source, 64);
            classify(
                source,
                &[target, target.color_exchanged()],
                GoalMatch::Exact,
                |goal| reachable.contains(&goal),
                None,
            );
            // An equivalence query already examines every goal image, so only
            // the identity source needs this check for each absolute color.
            if symmetry == BoardSymmetry::Identity {
                classify(
                    source,
                    &[target, target.color_exchanged()],
                    GoalMatch::EquivalenceClass,
                    |_| true,
                    None,
                );
            }
            assert!(reachable.contains(&target));
            assert!(!reachable.contains(&target.color_exchanged()));
        }
    }
}

#[test]
fn independent_start_and_goal_images_equal_fixed_start_orbit_query() {
    let initial = ArrayPosition::initial();
    for start in [initial, initial.apply(PlyAction::Play(19)).unwrap()] {
        let cap = start.discs() + 1;
        let fixed_start_reachable = enumerate(start, cap);
        let moved_start_reachable: Vec<_> =
            start.orbit().map(|image| enumerate(image, cap)).collect();
        let goals = query_targets(&fixed_start_reachable);
        classify(
            start,
            &goals,
            GoalMatch::SymmetryOrbit,
            |goal| {
                let independent_images = moved_start_reachable
                    .iter()
                    .any(|reachable| goal.orbit().any(|image| reachable.contains(&image)));
                let fixed_start = goal
                    .orbit()
                    .any(|image| fixed_start_reachable.contains(&image));
                assert_eq!(independent_images, fixed_start);
                independent_images
            },
            None,
        );
    }
}

#[test]
fn record_roundtrips_preserve_pass_tokens_and_reject_illegal_steps() {
    let start = middle_pass_start();
    let first = start.apply(PlyAction::Play(15)).unwrap();
    for (source, actions, text) in [
        (start, vec![], ""),
        (first, vec![PlyAction::Pass], "ps"),
        (first, vec![PlyAction::Pass, PlyAction::Play(55)], "ps H7"),
        (
            start,
            vec![PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)],
            "H2 ps H7",
        ),
        (start, vec![PlyAction::Play(15), PlyAction::Pass], "H2 ps"),
    ] {
        assert_eq!(format_actions(&actions), text);
        assert_eq!(parse_actions(text).unwrap(), actions);
        let record = exact_record(source, &actions);
        let independent: Vec<_> = source
            .replay(&actions)
            .unwrap()
            .into_iter()
            .map(ArrayPosition::position)
            .collect();
        assert_eq!(record.verify().unwrap(), independent);
    }

    let valid = exact_record(
        start,
        &[PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)],
    );
    for actions in [
        vec![PlyAction::Play(15), PlyAction::Play(55)], // missing middle pass
        vec![PlyAction::Pass, PlyAction::Play(15)],     // forbidden leading pass
        vec![PlyAction::Play(0)],                       // occupied square
        vec![PlyAction::Play(64)],                      // outside the board
        vec![
            PlyAction::Play(15),
            PlyAction::Pass,
            PlyAction::Play(55),
            PlyAction::Pass,
        ], // terminal pass
    ] {
        let mut invalid = valid.clone();
        invalid.actions = actions;
        assert!(invalid.verify().is_err());
    }
    let mut wrong_turn = valid;
    wrong_turn.goal.side_to_move = wrong_turn.goal.side_to_move.opposite();
    assert!(wrong_turn.verify().is_err());
    for malformed in ["psps", "A9", "I1", "A1B2", "foo"] {
        assert!(parse_actions(malformed).is_err());
    }
}

#[test]
fn recorded_symmetry_must_be_allowed_and_select_the_actual_endpoint() {
    let start = ArrayPosition::initial();
    let mut record = exact_record(start, &[]);
    record.transform.symmetry = BoardSymmetry::Rotate180;
    // Even an image equal to the endpoint is disallowed in an exact record.
    assert_eq!(start.transformed(record.transform.symmetry), start);
    assert!(record.verify().is_err());
    record.goal_match = GoalMatch::SymmetryOrbit;
    assert!(record.verify().is_ok());
    record.transform.symmetry = BoardSymmetry::Rotate90;
    assert_ne!(start.transformed(record.transform.symmetry), start);
    assert!(record.verify().is_err());
}

#[test]
fn parallel_duplicate_queries_each_keep_their_own_input_index() {
    let start = ArrayPosition::initial();
    let after_move = start.apply(PlyAction::Play(19)).unwrap();
    let wrong_turn = after_move.color_exchanged();
    let goals = [start, after_move, wrong_turn, after_move, start, wrong_turn];
    for goal_match in [GoalMatch::Exact, GoalMatch::SymmetryOrbit] {
        classify(start, &goals, goal_match, |goal| goal != wrong_turn, None);
    }
}

#[test]
fn nopass_witness_and_placement_columns_portfolio_agrees_with_the_array_oracle() {
    let start = ArrayPosition::initial();
    let reachable = enumerate(start, 6);
    let goals = query_targets(&reachable);
    assert!(goals.iter().any(|goal| reachable.contains(goal)));
    assert!(goals.iter().any(|goal| !reachable.contains(goal)));

    let positions = goals.iter().copied().map(ArrayPosition::position).collect();
    let portfolio = "nopass-witness+placement-columns"
        .parse::<SatPortfolio>()
        .unwrap();
    let mut callbacks = Vec::new();
    let outcomes = run_with_options(
        options(start.position(), positions, GoalMatch::Exact, portfolio),
        |outcome| {
            callbacks.push(outcome.input_index());
            Ok(())
        },
    )
    .unwrap();

    callbacks.sort_unstable();
    assert_eq!(callbacks, (1..=goals.len()).collect::<Vec<_>>());
    assert_eq!(outcomes.len(), goals.len());
    for (index, outcome) in outcomes.iter().enumerate() {
        let goal = goals[index];
        verify_answer(
            start,
            goal,
            GoalMatch::Exact,
            index + 1,
            outcome,
            reachable.contains(&goal),
        );
    }
}

#[test]
fn six_disc_queries_and_repeated_inputs_match_independent_placement_enumeration() {
    let start = ArrayPosition::initial();
    let reachable = enumerate_placements(start, 6);
    let mut goals = query_targets(&reachable);
    goals.extend([start, goals[0], start]);
    assert!(goals.iter().any(|goal| !reachable.contains(goal)));
    for mode in [
        GoalMatch::Exact,
        GoalMatch::SymmetryOrbit,
        GoalMatch::EquivalenceClass,
    ] {
        check_queries(start, &goals, mode);
    }
}

#[test]
fn seven_and_eight_disc_witnesses_preserve_colors_turns_and_arbitrary_starts() {
    let opening = ArrayPosition::initial().apply(PlyAction::Play(19)).unwrap();
    for start in [
        opening,
        opening
            .transformed(BoardSymmetry::Rotate90)
            .color_exchanged(),
    ] {
        let reachable = enumerate_placements(start, 8);
        let mut goals = vec![start, start.opposite_turn(), ArrayPosition::initial()];
        for discs in [7, 8] {
            let mut layer: Vec<_> = reachable
                .iter()
                .copied()
                .filter(|goal| goal.discs() == discs)
                .collect();
            layer.sort_by_key(|goal| goal.position());
            for index in [0, layer.len() / 2, layer.len() - 1] {
                let goal = layer[index];
                goals.extend([
                    goal,
                    goal.opposite_turn(),
                    goal.color_exchanged(),
                    goal.transformed(BoardSymmetry::Rotate90),
                ]);
            }
        }
        for mode in [
            GoalMatch::Exact,
            GoalMatch::SymmetryOrbit,
            GoalMatch::EquivalenceClass,
        ] {
            check_queries(start, &goals, mode);
        }
    }
}

#[test]
fn complete_portfolio_proves_unreachable_when_witness_only_is_unknown() {
    let start = ArrayPosition::initial();
    // White can play E3 from these colors, but Black must make the opening.
    // Give the resulting board the ordinary post-opening turn: occupation,
    // stone count, and turn parity are consistent, but no legal opening yields it.
    let goal = start
        .opposite_turn()
        .apply(PlyAction::Play(20))
        .unwrap()
        .opposite_turn();
    assert!(!enumerate_placements(start, 5).contains(&goal));

    let witness_outcomes = run_with_options(
        RunOptions {
            parallel_goals: 3,
            ..options(
                start.position(),
                vec![goal.position()],
                GoalMatch::Exact,
                SatEncoding::NoPassWitness,
            )
        },
        |_| Ok(()),
    )
    .unwrap();
    let [witness]: [GoalClassification; 1] = witness_outcomes.try_into().unwrap();
    assert!(matches!(witness, GoalClassification::Unknown { .. }));

    let portfolio = "nopass-witness+placement-columns"
        .parse::<SatPortfolio>()
        .unwrap();
    let portfolio_outcomes = run_with_options(
        RunOptions {
            parallel_goals: 3,
            ..options(
                start.position(),
                vec![goal.position()],
                GoalMatch::Exact,
                portfolio,
            )
        },
        |_| Ok(()),
    )
    .unwrap();
    let [portfolio]: [GoalClassification; 1] = portfolio_outcomes.try_into().unwrap();
    assert!(matches!(portfolio, GoalClassification::Unreachable { .. }));
}

#[test]
fn a_solver_deadline_never_produces_a_negative_reachability_proof() {
    let start = ArrayPosition::initial();
    let reachable = enumerate_placements(start, 8);
    let goal = *reachable
        .iter()
        .filter(|goal| goal.discs() == 8)
        .max_by_key(|goal| goal.position())
        .unwrap();
    let outcomes = run_with_options(
        RunOptions {
            parallel_goals: 3,
            sat_timeout_per_instance: Some(Duration::from_nanos(1)),
            ..options(
                start.position(),
                vec![goal.position()],
                GoalMatch::Exact,
                SatEncoding::NoPassWitness,
            )
        },
        |_| Ok(()),
    )
    .unwrap();
    match &outcomes[0] {
        GoalClassification::Reachable { .. } => {
            // A fast solver may finish before the timer thread is scheduled.
            verify_witness(start, goal, GoalMatch::Exact, &outcomes[0]);
        }
        GoalClassification::Unknown { .. } => {}
        GoalClassification::Unreachable { .. } => panic!("a timeout is not an unreachable proof"),
    }
}

#[test]
fn validate_options_rejects_invalid_settings_without_goals() {
    fn assert_error(opts: &RunOptions, expected: &str) {
        assert!(opts.goals.is_empty());
        assert_eq!(validate_options(opts).unwrap_err(), expected);
    }

    let mut opts = validation_options();
    opts.start.board = Board {
        player: 0,
        opponent: 0,
    };
    assert!(validate_options(&opts)
        .unwrap_err()
        .starts_with("invalid start:"));

    let mut opts = validation_options();
    opts.parallel_goals = 0;
    assert_error(&opts, "parallel_goals must be positive");

    let mut opts = validation_options();
    opts.seeds = 0;
    assert_error(&opts, "seeds must be positive");

    let mut opts = validation_options();
    opts.sat_timeout_per_instance = Some(Duration::ZERO);
    assert_error(&opts, "SAT timeout must be positive");

    let mut opts = validation_options();
    opts.cnf_dump_only = true;
    assert_error(&opts, "CNF-only runs require an output directory");

    let mut opts = validation_options();
    opts.encodings = SatEncoding::NoPassWitness.into();
    opts.reverse_bfs = Some(ReverseBfs {
        discs: 4,
        state_budget: 1,
    });
    assert_error(
        &opts,
        "reverse BFS requires a complete encoding in the portfolio; \
         nopass-witness alone cannot record its results",
    );

    let mut opts = validation_options();
    opts.reverse_bfs = Some(ReverseBfs {
        discs: 4,
        state_budget: 0,
    });
    assert_error(&opts, "reverse BFS state budget must be positive");

    for discs in [3, 65] {
        let mut opts = validation_options();
        opts.reverse_bfs = Some(ReverseBfs {
            discs,
            state_budget: 1,
        });
        assert_error(
            &opts,
            "reverse BFS discs must be between the start stone count and 64",
        );
    }
}

#[test]
fn validate_options_leaves_goal_validation_to_run_with_options() {
    let invalid_goal = Position {
        board: Board {
            player: 0,
            opponent: 0,
        },
        ..Position::initial()
    };
    let opts = options(
        Position::initial(),
        vec![invalid_goal],
        GoalMatch::Exact,
        SatEncoding::PlacementRays,
    );

    validate_options(&opts).unwrap();
    assert!(run_with_options(opts, |_| Ok(()))
        .unwrap_err()
        .starts_with("invalid goal[1]:"));
}

#[test]
fn run_with_options_validates_settings_before_goals() {
    let invalid_goal = Position {
        board: Board {
            player: 0,
            opponent: 0,
        },
        ..Position::initial()
    };
    let error = run_with_options(
        RunOptions {
            parallel_goals: 0,
            ..options(
                Position::initial(),
                vec![invalid_goal],
                GoalMatch::Exact,
                SatEncoding::PlacementRays,
            )
        },
        |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(error, "parallel_goals must be positive");
}

#[test]
fn sat_pipeline_settings_serialize_as_declared() {
    let portfolio =
        SatPortfolio::new(vec![SatEncoding::NoPassWitness, SatEncoding::PlacementRays]).unwrap();
    assert_eq!(
        serde_json::to_string(&portfolio).unwrap(),
        r#""nopass-witness+placement-rays""#
    );
    assert_eq!(
        serde_json::to_value(ReverseBfs {
            discs: 10,
            state_budget: 1_000_000,
        })
        .unwrap(),
        serde_json::json!({
            "discs": 10,
            "state_budget": 1_000_000,
        })
    );
}

#[test]
fn witness_only_portfolio_rejects_bfs_before_creating_output() {
    let directory = TestDirectory::new();
    let output = directory.path().join("witness-only");
    assert!(!output.exists());
    let run_options = RunOptions {
        parallel_goals: 1,
        cnf_dump_dir: Some(output.clone()),
        cnf_dump_only: true,
        reverse_bfs: Some(ReverseBfs {
            discs: 4,
            state_budget: 1,
        }),
        ..options(
            Position::initial(),
            vec![Position::initial()],
            GoalMatch::Exact,
            SatEncoding::NoPassWitness,
        )
    };

    let error = run_with_options(run_options, |_| Ok(())).unwrap_err();
    assert_eq!(
        error,
        "reverse BFS requires a complete encoding in the portfolio; \
         nopass-witness alone cannot record its results"
    );
    assert!(!output.exists());
}

#[test]
fn bfs_rejects_invalid_boundary_and_budget() {
    let error = run_with_options(
        RunOptions {
            parallel_goals: 1,
            reverse_bfs: Some(ReverseBfs {
                discs: 3,
                state_budget: 1,
            }),
            ..options(
                Position::initial(),
                vec![Position::initial()],
                GoalMatch::Exact,
                SatEncoding::PlacementRays,
            )
        },
        |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(
        error,
        "reverse BFS discs must be between the start stone count and 64"
    );

    let error = run_with_options(
        RunOptions {
            parallel_goals: 1,
            reverse_bfs: Some(ReverseBfs {
                discs: 4,
                state_budget: 0,
            }),
            ..options(
                Position::initial(),
                vec![Position::initial()],
                GoalMatch::Exact,
                SatEncoding::PlacementRays,
            )
        },
        |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(error, "reverse BFS state budget must be positive");
}

#[test]
fn goal_below_custom_start_boundary_is_unreachable() {
    let start = othello_complexity_rs::record::replay(Position::initial(), &[PlyAction::Play(19)])
        .unwrap()[1];
    assert_eq!(start.board.popcount(), 5);
    let goal = Position::initial();
    let outcomes = run_with_options(
        RunOptions {
            parallel_goals: 1,
            reverse_bfs: Some(ReverseBfs {
                discs: 5,
                state_budget: 1,
            }),
            ..options(
                start,
                vec![goal],
                GoalMatch::Exact,
                SatEncoding::PlacementRays,
            )
        },
        |_| Ok(()),
    )
    .unwrap();
    assert!(matches!(
        outcomes.as_slice(),
        [GoalClassification::Unreachable {
            goal: classified_goal,
            ..
        }] if *classified_goal == goal
    ));
}

/// Mid-game goals: reachable ones from random legal games, and perturbed
/// copies that are usually unreachable. Every encoding, with and without the
/// reverse BFS, must agree, and every witness must replay to its goal.
#[test]
fn encodings_and_bfs_agree_on_mid_game_goals() {
    use rand::{Rng, SeedableRng};

    let mut rng = rand::rngs::StdRng::seed_from_u64(0x6d69_6467);
    let start = Position::initial();
    let mut goals = Vec::new();
    for seed in 0..10 {
        let history = random_game(1000 + seed);
        let prefix = (6 + rng.random_range(0..6)).min(history.len() - 1);
        let (position, action) = history[prefix];
        let goal = othello_complexity_rs::record::replay(position, &[action]).unwrap()[1];
        goals.push(goal);
        let occupied = goal.board.player | goal.board.opponent;
        let mut perturbed = goal;
        for _ in 0..2 {
            let square = rng.random_range(0..64);
            let bit = 1_u64 << square;
            if occupied & bit != 0 {
                perturbed.board.player ^= bit;
                perturbed.board.opponent ^= bit;
            }
        }
        goals.push(perturbed);
        goals.push(Position {
            side_to_move: goal.side_to_move.opposite(),
            ..goal
        });
    }
    let mut reference: Option<Vec<bool>> = None;
    let mut reachable = 0;
    for encodings in [
        SatPortfolio::from(SatEncoding::PlacementColumns),
        SatPortfolio::from(SatEncoding::PlacementRays),
    ] {
        for bfs in [
            None,
            Some(ReverseBfs {
                discs: 8,
                state_budget: 500,
            }),
        ] {
            let outcomes = run_with_options(
                RunOptions {
                    parallel_goals: 2,
                    reverse_bfs: bfs,
                    ..options(start, goals.clone(), GoalMatch::Exact, encodings.clone())
                },
                |_| Ok(()),
            )
            .unwrap();
            let answers: Vec<bool> = outcomes
                .iter()
                .zip(&goals)
                .map(|(outcome, goal)| match outcome {
                    GoalClassification::Reachable { record } => {
                        assert_eq!(record.verify().unwrap().last(), Some(goal));
                        true
                    }
                    GoalClassification::Unreachable { .. } => false,
                    GoalClassification::Unknown { .. } => panic!("unbounded solve was unknown"),
                })
                .collect();
            match &reference {
                Some(expected) => assert_eq!(&answers, expected, "{encodings} {bfs:?}"),
                None => {
                    reachable = answers.iter().filter(|&&answer| answer).count();
                    reference = Some(answers);
                }
            }
        }
    }
    assert!(reachable >= 10, "reachable={reachable}");
    // Small goals keep this test quick; hardness grows quickly with size.
    assert!(goals.iter().all(|goal| goal.board.popcount() <= 18));
    assert!(reachable < goals.len());
}

/// Equivalence queries settle as soon as one candidate's reverse BFS finds a
/// witness, and unreachable classes need every candidate refuted.
#[test]
fn bfs_driver_matches_plain_sat_on_equivalence_queries() {
    let start = Position::initial();
    let mut goals = Vec::new();
    for seed in 0..6 {
        let history = random_game(2000 + seed);
        let (position, action) = history[(6 + seed as usize).min(history.len() - 1)];
        let goal = othello_complexity_rs::record::replay(position, &[action]).unwrap()[1];
        goals.push(goal);
        let mut perturbed = goal;
        let occupied = goal.board.player | goal.board.opponent;
        let bit = 1_u64 << (occupied.trailing_zeros() + 1);
        if occupied & bit != 0 {
            perturbed.board.player ^= bit;
            perturbed.board.opponent ^= bit;
        }
        goals.push(perturbed);
    }
    let witness_portfolio =
        SatPortfolio::new(vec![SatEncoding::NoPassWitness, SatEncoding::PlacementRays]).unwrap();
    let mut reference: Option<Vec<bool>> = None;
    for encodings in [
        SatPortfolio::from(SatEncoding::PlacementRays),
        witness_portfolio,
    ] {
        for bfs in [
            None,
            Some(ReverseBfs {
                discs: 8,
                state_budget: 200_000,
            }),
        ] {
            let outcomes = run_with_options(
                RunOptions {
                    parallel_goals: 2,
                    reverse_bfs: bfs,
                    ..options(
                        start,
                        goals.clone(),
                        GoalMatch::EquivalenceClass,
                        encodings.clone(),
                    )
                },
                |_| Ok(()),
            )
            .unwrap();
            let answers: Vec<bool> = outcomes
                .iter()
                .map(|outcome| match outcome {
                    GoalClassification::Reachable { record } => {
                        record.verify().unwrap();
                        true
                    }
                    GoalClassification::Unreachable { .. } => false,
                    GoalClassification::Unknown { .. } => panic!("unbounded solve was unknown"),
                })
                .collect();
            match &reference {
                Some(expected) => assert_eq!(&answers, expected, "{encodings} {bfs:?}"),
                None => reference = Some(answers),
            }
        }
    }
    let answers = reference.unwrap();
    assert!(answers.iter().any(|&answer| answer));
    assert!(answers.iter().any(|&answer| !answer));
}

#[test]
fn ordinary_cnf_is_nontrivial_for_every_encoding() {
    let start = ArrayPosition::initial();
    let goal = start.apply(PlyAction::Play(19)).unwrap();
    for encoding in SatEncoding::ALL {
        let (outcome, cnf) = dump_exact_cnf(encoding, start.position(), goal.position());
        assert!(
            matches!(outcome, GoalClassification::Reachable { .. }),
            "{encoding}"
        );
        assert!(cnf.variables > 0, "{encoding}: {cnf:?}");
        assert!(!cnf.clauses.is_empty(), "{encoding}: {cnf:?}");
        assert!(
            cnf.clauses.iter().all(|clause| !clause.is_empty()),
            "{encoding}: {cnf:?}"
        );
    }
}

#[test]
fn trivially_unreachable_cnf_contains_one_empty_clause_for_every_encoding() {
    let goal = ArrayPosition::initial();
    let start = goal.apply(PlyAction::Play(19)).unwrap();
    for encoding in SatEncoding::ALL
        .into_iter()
        .filter(|encoding| encoding.proves_unreachable())
    {
        let (outcome, cnf) = dump_exact_cnf(encoding, start.position(), goal.position());
        assert!(
            matches!(outcome, GoalClassification::Unreachable { .. }),
            "{encoding}"
        );
        assert_eq!(cnf.variables, 0, "{encoding}: {cnf:?}");
        assert_eq!(cnf.clauses, vec![Vec::<i32>::new()], "{encoding}");
    }
}

#[test]
fn zero_placement_reachable_cnf_is_empty_for_every_encoding() {
    let start = ArrayPosition::initial().position();
    for encoding in SatEncoding::ALL {
        let (outcome, cnf) = dump_exact_cnf(encoding, start, start);
        assert!(
            matches!(outcome, GoalClassification::Reachable { .. }),
            "{encoding}"
        );
        assert_eq!(cnf.variables, 0, "{encoding}: {cnf:?}");
        assert!(cnf.clauses.is_empty(), "{encoding}: {cnf:?}");
    }
}

#[test]
fn pass_only_reachable_cnf_is_empty_for_every_encoding() {
    let start = ArrayPosition::from_relative(
        "OXX-OOOOOXXXXXXOOXOOOXXOOOXOXOXXOXOXOOXXOOOXXXOXOOXOOOXXOOOOOOXX",
        false,
    );
    let goal = start.apply(PlyAction::Pass).unwrap();
    for encoding in SatEncoding::ALL
        .into_iter()
        .filter(|encoding| encoding.proves_unreachable())
    {
        let (outcome, cnf) = dump_exact_cnf(encoding, start.position(), goal.position());
        assert!(
            matches!(outcome, GoalClassification::Reachable { .. }),
            "{encoding}"
        );
        assert_eq!(cnf.variables, 0, "{encoding}: {cnf:?}");
        assert!(cnf.clauses.is_empty(), "{encoding}: {cnf:?}");
    }
}

#[test]
fn cnf_dump_only_writes_every_candidate_and_encoding_without_classifying() {
    let start = ArrayPosition::initial();
    let goals = [start, start.apply(PlyAction::Play(19)).unwrap()];
    let goal_positions: Vec<_> = goals.iter().map(|goal| goal.position()).collect();
    let encodings = [SatEncoding::NoPassWitness, SatEncoding::PlacementColumns];
    let portfolio = SatPortfolio::new(encodings.to_vec()).unwrap();
    let directory = TestDirectory::new();
    let run_options = RunOptions {
        cnf_dump_dir: Some(directory.path().to_path_buf()),
        cnf_dump_only: true,
        ..options(
            start.position(),
            goal_positions.clone(),
            GoalMatch::EquivalenceClass,
            portfolio,
        )
    };
    let mut callbacks = 0;
    let outcomes = run_with_options(run_options, |_| {
        callbacks += 1;
        Ok(())
    })
    .unwrap();
    assert!(outcomes.is_empty());
    assert_eq!(callbacks, 0);

    let mut expected_paths = Vec::new();
    for (index, &goal) in goal_positions.iter().enumerate() {
        for (transform, _) in candidates(start.position(), goal, GoalMatch::EquivalenceClass) {
            for encoding in encodings {
                expected_paths.push(
                    directory
                        .path()
                        .join(format!("goal_{:04}_{transform}_{encoding}.cnf", index + 1)),
                );
            }
        }
    }
    let actual_count = fs::read_dir(directory.path()).unwrap().count();
    assert_eq!(actual_count, expected_paths.len());
    for path in expected_paths {
        let text =
            fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        parse_dimacs(&text);
    }
}

#[test]
fn invalid_positions_and_callback_failures_are_execution_errors() {
    let start = Position::initial();
    let invalid_boards = [
        Board {
            player: 0,
            opponent: 0,
        },
        Board {
            player: start.board.player | start.board.opponent,
            opponent: start.board.opponent,
        },
    ];
    for encoding in SatEncoding::ALL {
        for board in invalid_boards {
            let invalid = Position { board, ..start };
            for (source, goals) in [(invalid, vec![start]), (start, vec![invalid])] {
                let mut callbacks = 0;
                let result =
                    run_with_options(options(source, goals, GoalMatch::Exact, encoding), |_| {
                        callbacks += 1;
                        Ok(())
                    });
                assert!(result.is_err(), "{encoding}");
                assert_eq!(callbacks, 0, "{encoding}");
            }
        }
        let empty = run_with_options(options(start, vec![], GoalMatch::Exact, encoding), |_| {
            panic!("empty input cannot produce an outcome")
        })
        .unwrap();
        assert!(empty.is_empty(), "{encoding}");
        assert!(
            run_with_options(
                options(start, vec![start], GoalMatch::Exact, encoding),
                |_| Err("intentional callback failure".to_string())
            )
            .is_err(),
            "{encoding}"
        );
    }
}

#[test]
fn invalid_input_is_rejected_before_creating_cnf_output() {
    let start = Position::initial();
    let invalid = Position {
        board: Board {
            player: 0,
            opponent: 0,
        },
        ..start
    };
    for encoding in SatEncoding::ALL {
        let directory = TestDirectory::new();
        let cnf_dir = directory.path().join("cnf");
        let run_options = RunOptions {
            cnf_dump_dir: Some(cnf_dir.clone()),
            ..options(invalid, vec![start], GoalMatch::Exact, encoding)
        };
        assert!(
            run_with_options(run_options, |_| Ok(())).is_err(),
            "{encoding}"
        );
        assert!(!cnf_dir.exists(), "{encoding}");
    }
}
