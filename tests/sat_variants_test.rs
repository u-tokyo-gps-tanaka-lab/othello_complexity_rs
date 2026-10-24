//! Compare all SAT configurations with the independent array rules.

mod support;

use std::collections::HashSet;

use othello_complexity_rs::filters::sat::{run_with_options, RunOptions, SatEncoding};
use othello_complexity_rs::othello::BoardSymmetry;
use othello_complexity_rs::query::GoalClassification;
use othello_complexity_rs::record::{GoalMatch, PlyAction};

use support::{enumerate, middle_pass_start, query_targets, ArrayPosition};

fn solve(encoding: SatEncoding, start: ArrayPosition, goal: ArrayPosition) -> GoalClassification {
    let options = RunOptions {
        start: start.position(),
        goals: vec![goal.position()],
        goal_match: GoalMatch::Exact,
        encoding,
        parallel_goals: 1,
        show_coords: false,
        show_boards: false,
        verbose: false,
        cnf_dump_dir: None,
        cnf_dump_only: false,
        sat_timeout_per_instance: None,
    };
    let outcomes = run_with_options(options, |_| Ok(())).unwrap_or_else(|error| {
        panic!("{encoding} failed for start={start:?}, goal={goal:?}: {error}")
    });
    let [outcome]: [GoalClassification; 1] = outcomes.try_into().unwrap();
    outcome
}

fn assert_all_variants(
    start: ArrayPosition,
    goal: ArrayPosition,
    expected_reachable: bool,
    expected_actions: Option<&[PlyAction]>,
) {
    for encoding in SatEncoding::ALL {
        match solve(encoding, start, goal) {
            GoalClassification::Reachable { record } => {
                assert!(
                    expected_reachable,
                    "{encoding} accepted an unreachable goal: start={start:?}, goal={goal:?}"
                );
                let independent_trace = start.replay(&record.actions).unwrap_or_else(|error| {
                    panic!(
                        "{encoding} returned an illegal certificate {:?}: {error}",
                        record.actions
                    )
                });
                assert_eq!(independent_trace.last(), Some(&goal), "{encoding}");
                if let Some(expected) = expected_actions {
                    assert_eq!(record.actions.as_slice(), expected, "{encoding}");
                }
                let independent_positions: Vec<_> = independent_trace
                    .into_iter()
                    .map(ArrayPosition::position)
                    .collect();
                assert_eq!(record.verify().unwrap(), independent_positions, "{encoding}");
            }
            GoalClassification::Unreachable { .. } => assert!(
                !expected_reachable,
                "{encoding} rejected a reachable goal: start={start:?}, goal={goal:?}"
            ),
            GoalClassification::Unknown { .. } => panic!(
                "{encoding} returned UNKNOWN for an unbounded small query: start={start:?}, goal={goal:?}"
            ),
        }
    }
}

fn targets_with_turn_changes(reachable: &HashSet<ArrayPosition>) -> Vec<ArrayPosition> {
    let mut goals = query_targets(reachable);
    let opposite_turns: Vec<_> = goals.iter().map(|goal| goal.opposite_turn()).collect();
    goals.extend(opposite_turns);
    goals.sort_by_key(|goal| goal.position());
    goals.dedup();
    goals
}

#[test]
fn all_six_disc_queries_match_independent_enumeration() {
    let start = ArrayPosition::initial();
    let reachable = enumerate(start, 6);
    let goals = targets_with_turn_changes(&reachable);
    assert!(goals.iter().any(|goal| !reachable.contains(goal)));
    for goal in goals {
        assert_all_variants(start, goal, reachable.contains(&goal), None);
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
    for goal in goals {
        assert_all_variants(start, goal, reachable.contains(&goal), None);
    }
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
        for (source, goal, expected) in [
            (start, terminal, &actions[..]),
            (first, terminal, &actions[1..]),
            (start, after_pass, &actions[..2]),
            (first, after_pass, &actions[1..2]),
            (after_pass, terminal, &actions[2..]),
        ] {
            assert_all_variants(source, goal, true, Some(expected));
        }
        let reachable = enumerate(start, 64);
        let wrong_terminal_turn = terminal.opposite_turn();
        assert!(!reachable.contains(&wrong_terminal_turn));
        assert_all_variants(start, wrong_terminal_turn, false, None);
        assert_all_variants(terminal, terminal, true, Some(&[]));
        assert_all_variants(terminal, wrong_terminal_turn, false, None);
    }
}

#[test]
fn zero_placements_allow_identity_or_one_legal_pass_only() {
    let initial = ArrayPosition::initial();
    assert_all_variants(initial, initial, true, Some(&[]));
    assert!(initial.apply(PlyAction::Pass).is_err());
    assert_all_variants(initial, initial.opposite_turn(), false, None);

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
        assert_all_variants(start, start, true, Some(&[]));
        assert_all_variants(start, after_pass, true, Some(&[PlyAction::Pass]));
        assert!(after_pass.apply(PlyAction::Pass).is_err());
        assert_all_variants(after_pass, start, false, None);
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
        assert_all_variants(terminal, terminal, true, Some(&[]));
        assert_all_variants(terminal, terminal.opposite_turn(), false, None);
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
        assert_all_variants(start, goal, true, Some(&actions));
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
    assert_all_variants(start, false_leading_pass_goal, false, None);

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
    assert_all_variants(start, false_trailing_pass_goal, false, None);
}
