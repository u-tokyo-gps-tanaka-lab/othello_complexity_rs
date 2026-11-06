//! The SAT driver is checked against the independent array-based rules oracle.
//! The nopass-witness encoding can prove reachability, but not unreachability.

mod support;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use othello_complexity_rs::filters::sat::{
    run_with_options, validate_options, RunOptions, SatEncoding, SatPortfolio,
};
use othello_complexity_rs::othello::{Board, BoardSymmetry, Position};
use othello_complexity_rs::query::{candidates, GoalClassification};
use othello_complexity_rs::record::{GoalMatch, PlyAction, PositionTransform};
use othello_complexity_rs::search::{run_strategy, Strategy};

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

#[test]
fn placement_encodings_agree_with_enumeration_on_six_disc_queries() {
    let start = ArrayPosition::initial();
    let reachable = enumerate(start, 6);
    let goals = targets_with_turn_changes(&reachable);
    assert!(goals.iter().any(|goal| !reachable.contains(goal)));
    let goal_positions = goals.iter().map(|goal| goal.position()).collect::<Vec<_>>();
    for encoding in [SatEncoding::PlacementColumns, SatEncoding::PlacementRays] {
        let outcomes = run_with_options(
            RunOptions {
                parallel_goals: 2,
                ..options(
                    start.position(),
                    goal_positions.clone(),
                    GoalMatch::Exact,
                    encoding,
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
                    assert!(expected, "{encoding}: {goal:?}");
                    assert_eq!(record.verify().unwrap().last(), Some(&goal.position()));
                }
                GoalClassification::Unreachable { .. } => {
                    assert!(!expected, "{encoding}: {goal:?}");
                }
                GoalClassification::Unknown { .. } => {
                    panic!("{encoding}: unknown for {goal:?}")
                }
            }
        }
    }
}

#[test]
fn all_seven_disc_targets_and_both_turns_agree_with_independent_enumeration() {
    let start = ArrayPosition::initial();
    let reachable = enumerate(start, 7);
    let mut goals = query_targets(&reachable);
    let monochromatic = ArrayPosition::from_relative(
        "-------------------X-------XX------XX---------------------------",
        true,
    );
    goals.push(monochromatic);
    goals.extend([start, goals[0], start]);
    assert!(goals.iter().any(|goal| !reachable.contains(goal)));
    for goal_match in [GoalMatch::Exact, GoalMatch::EquivalenceClass] {
        classify(
            start,
            &goals,
            goal_match,
            |goal| match goal_match {
                GoalMatch::Exact => reachable.contains(&goal),
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
    for mode in [GoalMatch::Exact, GoalMatch::EquivalenceClass] {
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
        for mode in [GoalMatch::Exact, GoalMatch::EquivalenceClass] {
            check_queries(start, &goals, mode);
        }
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
}

#[test]
fn sat_pipeline_settings_serialize_as_declared() {
    let portfolio =
        SatPortfolio::new(vec![SatEncoding::NoPassWitness, SatEncoding::PlacementRays]).unwrap();
    assert_eq!(
        serde_json::to_string(&portfolio).unwrap(),
        r#""nopass-witness+placement-rays""#
    );
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

#[test]
fn bfs_stage_agrees_with_sat_on_decided_exact_and_equivalence_mid_game_goals() {
    use rand::{Rng, SeedableRng};

    let mut rng = rand::rngs::StdRng::seed_from_u64(0x6d69_6467);
    let start = Position::initial();
    let mut exact_goals = Vec::new();
    for seed in 0..10 {
        let history = random_game(1000 + seed);
        let prefix = (6 + rng.random_range(0..6)).min(history.len() - 1);
        let (position, action) = history[prefix];
        let goal = othello_complexity_rs::record::replay(position, &[action]).unwrap()[1];
        exact_goals.push(goal);
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
        exact_goals.push(perturbed);
        exact_goals.push(Position {
            side_to_move: goal.side_to_move.opposite(),
            ..goal
        });
    }
    let mut equivalence_goals = Vec::new();
    for seed in 0..6 {
        let history = random_game(2000 + seed);
        let (position, action) = history[(6 + seed as usize).min(history.len() - 1)];
        let goal = othello_complexity_rs::record::replay(position, &[action]).unwrap()[1];
        equivalence_goals.push(goal);
        let mut perturbed = goal;
        let occupied = goal.board.player | goal.board.opponent;
        let bit = 1_u64 << (occupied.trailing_zeros() + 1);
        if occupied & bit != 0 {
            perturbed.board.player ^= bit;
            perturbed.board.opponent ^= bit;
        }
        equivalence_goals.push(perturbed);
    }
    let witness_portfolio =
        SatPortfolio::new(vec![SatEncoding::NoPassWitness, SatEncoding::PlacementRays]).unwrap();

    struct Case {
        name: &'static str,
        goals: Vec<Position>,
        goal_match: GoalMatch,
        portfolios: Vec<SatPortfolio>,
        bfs_discs: u32,
        bfs_max_nodes: usize,
        minimum_reachable: usize,
        maximum_discs: Option<u32>,
        verify_exact_endpoint: bool,
    }

    let cases = [
        Case {
            name: "exact",
            goals: exact_goals,
            goal_match: GoalMatch::Exact,
            portfolios: vec![
                SatEncoding::PlacementColumns.into(),
                SatEncoding::PlacementRays.into(),
            ],
            bfs_discs: 8,
            bfs_max_nodes: 500,
            minimum_reachable: 10,
            maximum_discs: Some(18),
            verify_exact_endpoint: true,
        },
        Case {
            name: "equivalence",
            goals: equivalence_goals,
            goal_match: GoalMatch::EquivalenceClass,
            portfolios: vec![SatEncoding::PlacementRays.into(), witness_portfolio],
            bfs_discs: 8,
            bfs_max_nodes: 200_000,
            minimum_reachable: 1,
            maximum_discs: None,
            verify_exact_endpoint: false,
        },
    ];

    for case in cases {
        let mut reference: Option<Vec<bool>> = None;
        for encodings in case.portfolios {
            let outcomes = run_with_options(
                RunOptions {
                    parallel_goals: 2,
                    ..options(
                        start,
                        case.goals.clone(),
                        case.goal_match,
                        encodings.clone(),
                    )
                },
                |_| Ok(()),
            )
            .unwrap_or_else(|error| panic!("{}: {encodings}: {error}", case.name));
            let answers: Vec<bool> = outcomes
                .iter()
                .zip(&case.goals)
                .map(|(outcome, goal)| match outcome {
                    GoalClassification::Reachable { record } => {
                        let trace = record.verify().unwrap();
                        if case.verify_exact_endpoint {
                            assert_eq!(trace.last(), Some(goal), "{}: {encodings}", case.name);
                        }
                        true
                    }
                    GoalClassification::Unreachable { .. } => false,
                    GoalClassification::Unknown { .. } => {
                        panic!("{}: unbounded solve was unknown", case.name)
                    }
                })
                .collect();
            match &reference {
                Some(expected) => {
                    assert_eq!(&answers, expected, "{}: {encodings}", case.name)
                }
                None => reference = Some(answers),
            }
        }
        let answers = reference.unwrap();
        let reachable = answers.iter().filter(|&&answer| answer).count();
        assert!(
            reachable >= case.minimum_reachable,
            "{}: reachable={reachable}",
            case.name
        );
        assert!(reachable < case.goals.len(), "{}", case.name);
        if let Some(maximum) = case.maximum_discs {
            assert!(
                case.goals
                    .iter()
                    .all(|goal| goal.board.popcount() <= maximum),
                "{}",
                case.name
            );
        }

        let directory = TestDirectory::new();
        let bfs = run_strategy(
            &Strategy::Bfs {
                discs: case.bfs_discs,
                max_nodes: case.bfs_max_nodes,
                use_lp: false,
                threads: 2,
                parallel_goals: 2,
            },
            &case.goals,
            &directory.path().join("bfs"),
            case.goal_match,
        )
        .unwrap_or_else(|error| panic!("{}: BFS: {error}", case.name));
        let mut bfs_ok = 0;
        let mut bfs_ng = 0;
        for ((classification, _), expected) in bfs.iter().zip(answers) {
            match classification {
                GoalClassification::Reachable { record } => {
                    assert!(expected, "{}: BFS contradicted SAT", case.name);
                    record.verify().unwrap();
                    bfs_ok += 1;
                }
                GoalClassification::Unreachable { .. } => {
                    assert!(!expected, "{}: BFS contradicted SAT", case.name);
                    bfs_ng += 1;
                }
                GoalClassification::Unknown { .. } => {}
            }
        }
        assert!(bfs_ok > 0, "{}: BFS decided no OK query", case.name);
        assert!(bfs_ng > 0, "{}: BFS decided no NG query", case.name);
    }
}

#[test]
fn cnf_shape_matches_query_classification_for_every_encoding() {
    #[derive(Clone, Copy)]
    enum ExpectedClassification {
        Reachable,
        Unreachable,
    }

    #[derive(Clone, Copy)]
    enum ExpectedShape {
        Nontrivial,
        Empty,
        OneEmptyClause,
    }

    struct Case {
        name: &'static str,
        start: Position,
        goal: Position,
        encodings: Vec<SatEncoding>,
        classification: ExpectedClassification,
        shape: ExpectedShape,
    }

    let initial = ArrayPosition::initial();
    let after_opening = initial.apply(PlyAction::Play(19)).unwrap();
    let before_pass = ArrayPosition::from_relative(
        "OXX-OOOOOXXXXXXOOXOOOXXOOOXOXOXXOXOXOOXXOOOXXXOXOOXOOOXXOOOOOOXX",
        false,
    );
    let after_pass = before_pass.apply(PlyAction::Pass).unwrap();
    let complete_encodings = || {
        SatEncoding::ALL
            .into_iter()
            .filter(|encoding| encoding.proves_unreachable())
            .collect()
    };
    let cases = [
        Case {
            name: "ordinary",
            start: initial.position(),
            goal: after_opening.position(),
            encodings: SatEncoding::ALL.to_vec(),
            classification: ExpectedClassification::Reachable,
            shape: ExpectedShape::Nontrivial,
        },
        Case {
            name: "stone-count-decrease",
            start: after_opening.position(),
            goal: initial.position(),
            encodings: complete_encodings(),
            classification: ExpectedClassification::Unreachable,
            shape: ExpectedShape::OneEmptyClause,
        },
        Case {
            name: "zero-placement",
            start: initial.position(),
            goal: initial.position(),
            encodings: SatEncoding::ALL.to_vec(),
            classification: ExpectedClassification::Reachable,
            shape: ExpectedShape::Empty,
        },
        Case {
            name: "pass-only",
            start: before_pass.position(),
            goal: after_pass.position(),
            encodings: complete_encodings(),
            classification: ExpectedClassification::Reachable,
            shape: ExpectedShape::Empty,
        },
    ];

    for case in cases {
        for encoding in case.encodings {
            let (outcome, cnf) = dump_exact_cnf(encoding, case.start, case.goal);
            match case.classification {
                ExpectedClassification::Reachable => assert!(
                    matches!(outcome, GoalClassification::Reachable { .. }),
                    "{}: {encoding}",
                    case.name
                ),
                ExpectedClassification::Unreachable => assert!(
                    matches!(outcome, GoalClassification::Unreachable { .. }),
                    "{}: {encoding}",
                    case.name
                ),
            }
            match case.shape {
                ExpectedShape::Nontrivial => {
                    assert!(cnf.variables > 0, "{}: {encoding}: {cnf:?}", case.name);
                    assert!(
                        !cnf.clauses.is_empty(),
                        "{}: {encoding}: {cnf:?}",
                        case.name
                    );
                    assert!(
                        cnf.clauses.iter().all(|clause| !clause.is_empty()),
                        "{}: {encoding}: {cnf:?}",
                        case.name
                    );
                }
                ExpectedShape::Empty => {
                    assert_eq!(cnf.variables, 0, "{}: {encoding}: {cnf:?}", case.name);
                    assert!(cnf.clauses.is_empty(), "{}: {encoding}: {cnf:?}", case.name);
                }
                ExpectedShape::OneEmptyClause => {
                    assert_eq!(cnf.variables, 0, "{}: {encoding}: {cnf:?}", case.name);
                    assert_eq!(
                        cnf.clauses,
                        vec![Vec::<i32>::new()],
                        "{}: {encoding}",
                        case.name
                    );
                }
            }
        }
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
fn invalid_positions_empty_goals_and_callback_failures_have_declared_results() {
    enum Expected {
        ErrorPrefix(&'static str),
        Empty,
        CallbackError(&'static str),
    }

    struct Case {
        name: &'static str,
        start: Position,
        goals: Vec<Position>,
        callback_error: Option<&'static str>,
        expected: Expected,
        expected_callbacks: usize,
    }

    let start = Position::initial();
    let empty = Position {
        board: Board {
            player: 0,
            opponent: 0,
        },
        ..start
    };
    let overlapping = Position {
        board: Board {
            player: start.board.player | start.board.opponent,
            opponent: start.board.opponent,
        },
        ..start
    };
    let cases = [
        Case {
            name: "empty-board-start",
            start: empty,
            goals: vec![start],
            callback_error: None,
            expected: Expected::ErrorPrefix("invalid start:"),
            expected_callbacks: 0,
        },
        Case {
            name: "overlapping-start",
            start: overlapping,
            goals: vec![start],
            callback_error: None,
            expected: Expected::ErrorPrefix("invalid start:"),
            expected_callbacks: 0,
        },
        Case {
            name: "empty-board-goal",
            start,
            goals: vec![empty],
            callback_error: None,
            expected: Expected::ErrorPrefix("invalid goal[1]:"),
            expected_callbacks: 0,
        },
        Case {
            name: "overlapping-goal",
            start,
            goals: vec![overlapping],
            callback_error: None,
            expected: Expected::ErrorPrefix("invalid goal[1]:"),
            expected_callbacks: 0,
        },
        Case {
            name: "empty-goals",
            start,
            goals: Vec::new(),
            callback_error: None,
            expected: Expected::Empty,
            expected_callbacks: 0,
        },
        Case {
            name: "callback-error",
            start,
            goals: vec![start],
            callback_error: Some("intentional callback failure"),
            expected: Expected::CallbackError("intentional callback failure"),
            expected_callbacks: 1,
        },
    ];

    for case in cases {
        let mut callbacks = 0;
        let result = run_with_options(
            options(
                case.start,
                case.goals,
                GoalMatch::Exact,
                SatEncoding::PlacementRays,
            ),
            |_| {
                callbacks += 1;
                match case.callback_error {
                    Some(error) => Err(error.to_owned()),
                    None => Ok(()),
                }
            },
        );
        match case.expected {
            Expected::ErrorPrefix(prefix) => {
                let error = result.unwrap_err();
                assert!(error.starts_with(prefix), "{}: {error}", case.name);
            }
            Expected::Empty => assert!(result.unwrap().is_empty(), "{}", case.name),
            Expected::CallbackError(expected) => {
                assert_eq!(result.unwrap_err(), expected, "{}", case.name)
            }
        }
        assert_eq!(callbacks, case.expected_callbacks, "{}", case.name);
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
