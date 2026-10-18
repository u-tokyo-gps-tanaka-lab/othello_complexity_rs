mod support;

use std::collections::HashSet;

use othello_complexity_rs::othello::{Board, BoardSymmetry, Position};
use othello_complexity_rs::prunings::layer_sat::{
    run_with_options, GoalClassification, RunOptions,
};
use othello_complexity_rs::record::{
    format_actions, parse_actions, GoalMatch, PlyAction, ReachabilityRecord,
};

use support::{enumerate, middle_pass_start, query_targets, ArrayPosition, ALL_SYMMETRIES};

fn options(start: Position, goals: Vec<Position>, goal_match: GoalMatch) -> RunOptions {
    RunOptions {
        start,
        goals,
        goal_match,
        parallel_goals: 4,
        show_coords: false,
        show_boards: false,
        verbose: false,
        cnf_dump_dir: None,
        cnf_dump_only: false,
        sat_timeout_per_instance: None,
    }
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
                assert_eq!(record.symmetry, BoardSymmetry::Identity);
            }
            let independent_trace = start.replay(&record.actions).unwrap();
            let endpoint = *independent_trace.last().unwrap();
            assert_eq!(endpoint, goal.transformed(record.symmetry));
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
) -> Vec<GoalClassification> {
    let positions: Vec<_> = goals.iter().copied().map(ArrayPosition::position).collect();
    let mut callbacks = Vec::new();
    let outcomes = run_with_options(options(start.position(), positions, goal_match), |result| {
        callbacks.push(result.input_index());
        Ok(())
    })
    .unwrap();
    assert_eq!(outcomes.len(), goals.len());
    callbacks.sort_unstable();
    assert_eq!(callbacks, (1..=goals.len()).collect::<Vec<_>>());
    let mut returned_indices = HashSet::new();
    for outcome in &outcomes {
        let input_index = outcome.input_index();
        assert!((1..=goals.len()).contains(&input_index));
        assert!(returned_indices.insert(input_index));
        let goal = goals[input_index - 1];
        verify_answer(
            start,
            goal,
            goal_match,
            input_index,
            outcome,
            expected(goal),
        );
    }
    outcomes
}

fn exact_record(start: ArrayPosition, actions: &[PlyAction]) -> ReachabilityRecord {
    let trace = start.replay(actions).unwrap();
    ReachabilityRecord {
        input_index: 1,
        start: start.position(),
        goal: trace.last().unwrap().position(),
        goal_match: GoalMatch::Exact,
        symmetry: BoardSymmetry::Identity,
        actions: actions.to_vec(),
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

#[test]
fn all_seven_disc_targets_and_both_turns_agree_with_independent_enumeration() {
    let start = ArrayPosition::initial();
    let reachable = enumerate(start, 7);
    let goals = query_targets(&reachable);
    assert!(goals.iter().any(|goal| !reachable.contains(goal)));
    for goal_match in [GoalMatch::Exact, GoalMatch::SymmetryOrbit] {
        classify(start, &goals, goal_match, |goal| match goal_match {
            GoalMatch::Exact => reachable.contains(&goal),
            GoalMatch::SymmetryOrbit => goal.orbit().any(|image| reachable.contains(&image)),
        });
    }
}

#[test]
fn zero_length_and_reflected_initial_queries_preserve_absolute_turn() {
    let start = ArrayPosition::initial();
    let reflected = start.transformed(BoardSymmetry::MirrorHorizontal);
    assert_ne!(start, reflected);
    let goals = [
        start,
        start.color_exchanged(),
        reflected,
        reflected.color_exchanged(),
    ];
    let exact = classify(start, &goals, GoalMatch::Exact, |goal| goal == start);
    let symmetric = classify(start, &goals, GoalMatch::SymmetryOrbit, |goal| {
        goal == start || goal == reflected
    });
    for outcome in exact.iter().chain(&symmetric) {
        if let Some(record) = outcome.record() {
            assert!(record.actions.is_empty());
        }
    }
}

#[test]
fn stone_count_cannot_decrease_and_opening_move_changes_turn() {
    let start = ArrayPosition::initial();
    let after_move = start.apply(PlyAction::Play(19)).unwrap();
    classify(
        start,
        &[after_move, after_move.color_exchanged()],
        GoalMatch::Exact,
        |goal| goal == after_move,
    );
    for goal_match in [GoalMatch::Exact, GoalMatch::SymmetryOrbit] {
        classify(after_move, &[start], goal_match, |_| false);
    }
}

#[test]
fn known_single_pass_is_legal_in_both_absolute_colors() {
    let fixture = ArrayPosition::from_relative(
        "OXX-OOOOOXXXXXXOOXOOOXXOOOXOXOXXOXOXOOXXOOOXXXOXOOXOOOXXOOOOOOXX",
        false,
    );
    for start in [fixture, fixture.color_exchanged()] {
        assert!(start.moves().is_empty());
        let after_pass = start.apply(PlyAction::Pass).unwrap();
        assert_eq!(after_pass.moves(), vec![3]); // D1
        let outcomes = classify(
            start,
            &[start, after_pass, after_pass.color_exchanged()],
            GoalMatch::Exact,
            |goal| goal == start || goal == after_pass,
        );
        let pass_record = outcomes
            .iter()
            .find(|outcome| outcome.input_index() == 2)
            .unwrap()
            .record()
            .unwrap();
        assert_eq!(pass_record.actions, vec![PlyAction::Pass]);

        // The enumeration cap stops placements, not the required pass.
        let at_cap = enumerate(start, start.discs());
        assert_eq!(at_cap, HashSet::from([start, after_pass]));
    }
}

#[test]
fn full_and_nonfull_terminal_positions_allow_only_zero_length() {
    for start in [
        ArrayPosition::from_relative(&"X".repeat(64), true),
        ArrayPosition::from_relative(
            "---------------------------XX------XX---------------------------",
            true,
        ),
    ] {
        assert!(start.moves().is_empty());
        assert!(start.apply(PlyAction::Pass).is_err());
        let swapped = start.opposite_turn();
        for goal_match in [GoalMatch::Exact, GoalMatch::SymmetryOrbit] {
            classify(start, &[start, swapped], goal_match, |goal| goal == start);
        }
    }
}

#[test]
fn leading_middle_trailing_and_only_passes_have_verified_sat_records() {
    let start = middle_pass_start();
    let first = start.apply(PlyAction::Play(15)).unwrap();
    let after_pass = first.apply(PlyAction::Pass).unwrap();
    let terminal = after_pass.apply(PlyAction::Play(55)).unwrap();
    assert_eq!(start.discs(), 62);
    assert_eq!(start.moves(), vec![15]);
    assert!(first.moves().is_empty());
    assert_eq!(after_pass.moves(), vec![55]);
    assert!(terminal.apply(PlyAction::Pass).is_err());
    for (source, target, expected_actions) in [
        (first, terminal, vec![PlyAction::Pass, PlyAction::Play(55)]),
        (
            start,
            terminal,
            vec![PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)],
        ),
        (
            start,
            after_pass,
            vec![PlyAction::Play(15), PlyAction::Pass],
        ),
        (first, after_pass, vec![PlyAction::Pass]),
    ] {
        let outcomes = classify(source, &[target], GoalMatch::Exact, |_| true);
        assert_eq!(outcomes[0].record().unwrap().actions, expected_actions);
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
    record.symmetry = BoardSymmetry::Rotate180;
    // Even an image equal to the endpoint is disallowed in an exact record.
    assert_eq!(start.transformed(record.symmetry), start);
    assert!(record.verify().is_err());
    record.goal_match = GoalMatch::SymmetryOrbit;
    assert!(record.verify().is_ok());
    record.symmetry = BoardSymmetry::Rotate90;
    assert_ne!(start.transformed(record.symmetry), start);
    assert!(record.verify().is_err());
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
            );
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
        classify(start, &goals, GoalMatch::SymmetryOrbit, |goal| {
            let independent_images = moved_start_reachable
                .iter()
                .any(|reachable| goal.orbit().any(|image| reachable.contains(&image)));
            let fixed_start = goal
                .orbit()
                .any(|image| fixed_start_reachable.contains(&image));
            assert_eq!(independent_images, fixed_start);
            independent_images
        });
    }
}

#[test]
fn parallel_duplicate_queries_each_keep_their_own_input_index() {
    let start = ArrayPosition::initial();
    let after_move = start.apply(PlyAction::Play(19)).unwrap();
    let wrong_turn = after_move.color_exchanged();
    let goals = [start, after_move, wrong_turn, after_move, start, wrong_turn];
    for goal_match in [GoalMatch::Exact, GoalMatch::SymmetryOrbit] {
        classify(start, &goals, goal_match, |goal| goal != wrong_turn);
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
    for board in invalid_boards {
        let invalid = Position { board, ..start };
        for (source, goals) in [(invalid, vec![start]), (start, vec![invalid])] {
            let mut callbacks = 0;
            let result = run_with_options(options(source, goals, GoalMatch::Exact), |_| {
                callbacks += 1;
                Ok(())
            });
            assert!(result.is_err());
            assert_eq!(callbacks, 0);
        }
    }
    assert!(run_with_options(options(start, vec![], GoalMatch::Exact), |_| Ok(())).is_err());
    assert!(
        run_with_options(options(start, vec![start], GoalMatch::Exact), |_| {
            Err("intentional callback failure".to_string())
        })
        .is_err()
    );
}
