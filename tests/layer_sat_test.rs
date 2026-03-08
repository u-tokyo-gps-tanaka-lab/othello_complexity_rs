use std::collections::HashSet;

use othello_complexity_rs::io::parse_line_to_board;
use othello_complexity_rs::othello::{flip, get_moves, Board};
use othello_complexity_rs::prunings::layer_sat::{
    run_with_options, GoalClassification, RunOptions,
};

fn classify_single_goal(
    start: Board,
    goal: Board,
    check_symmetry: bool,
    start_turn_black: bool,
) -> GoalClassification {
    let mut outcomes = run_with_options(
        RunOptions {
            start,
            goals: vec![goal],
            check_symmetry,
            start_turn_black,
            parallel_goals: 1,
            show_coords: false,
            show_boards: false,
            verbose: false,
            cnf_dump_dir: None,
            cnf_dump_only: false,
            sat_timeout_per_depth: None,
        },
        |_| Ok(()),
    )
    .unwrap();

    assert_eq!(outcomes.len(), 1);
    let outcome = outcomes.pop().unwrap();
    assert_eq!(outcome.goal(), goal);
    outcome
}

fn enumerate_reachable_states(max_discs: u32) -> HashSet<(Board, bool)> {
    let mut seen = HashSet::new();
    let mut stack = vec![(Board::initial(), true)];

    while let Some((board, turn_black)) = stack.pop() {
        if board.popcount() > max_discs || !seen.insert((board, turn_black)) {
            continue;
        }

        let moves = get_moves(board.player, board.opponent);
        if moves == 0 {
            let reply_moves = get_moves(board.opponent, board.player);
            if reply_moves != 0 {
                stack.push((board.swapped(), !turn_black));
            }
            continue;
        }
        if board.popcount() == max_discs {
            continue;
        }

        let mut moves = moves;
        while moves != 0 {
            let sq = moves.trailing_zeros() as usize;
            moves &= moves - 1;
            let flipped = flip(sq, board.player, board.opponent);
            let next = Board::new(
                board.opponent ^ flipped,
                board.player ^ (flipped | (1_u64 << sq)),
            );
            stack.push((next, !turn_black));
        }
    }

    seen
}

#[test]
fn test_single_pass() {
    let start =
        parse_line_to_board("OXX-OOOOOXXXXXXOOXOOOXXOOOXOXOXXOXOXOOXXOOOXXXOXOOXOOOXXOOOOOOXX")
            .unwrap();
    let goal =
        parse_line_to_board("XOO-XXXXXOOOOOOXXOXXXOOXXXOXOXOOXOXOXXOOXXXOOOXOXXOXXXOOXXXXXXOO")
            .unwrap();

    assert!(matches!(
        classify_single_goal(start, goal, false, false),
        GoalClassification::Reachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, goal, false, true),
        GoalClassification::Reachable { .. }
    ));
}

#[test]
fn test_terminal_pass_is_rejected() {
    let start = Board::new(u64::MAX, 0);
    let goal = start.swapped();

    assert!(matches!(
        classify_single_goal(start, goal, false, true),
        GoalClassification::Unreachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, start, false, true),
        GoalClassification::Reachable { .. }
    ));
}

#[test]
fn test_symmetry_1() {
    let start =
        parse_line_to_board("-----------O------OO------XOX------XOX-------O------------------")
            .unwrap();
    let goal =
        parse_line_to_board("------------------XO----OOOOO------OX-------OX------------------")
            .unwrap();

    assert!(matches!(
        classify_single_goal(start, goal, false, true),
        GoalClassification::Unreachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, goal, true, true),
        GoalClassification::Reachable { .. }
    ));
}

#[test]
fn test_symmetry_2() {
    let enable_check_symmetry = true;
    let start = Board::initial();
    let goal =
        parse_line_to_board("---------------------------OOO-----XO---------------------------")
            .unwrap();

    assert!(matches!(
        classify_single_goal(start, goal, !enable_check_symmetry, true),
        GoalClassification::Unreachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, goal, enable_check_symmetry, true),
        GoalClassification::Reachable { .. }
    ));
}

#[test]
fn test_symmetry_3() {
    let start = Board::initial();

    // 白手番(O)
    let goal_d3 =
        parse_line_to_board("-------------------X-------XX------XO---------------------------")
            .unwrap();
    let goal_c4 =
        parse_line_to_board("--------------------------XXX------XO---------------------------")
            .unwrap();
    let goal_e6 =
        parse_line_to_board("---------------------------OX------XX-------X-------------------")
            .unwrap();
    let goal_f5 =
        parse_line_to_board("---------------------------OX------XXX--------------------------")
            .unwrap();

    assert!(matches!(
        classify_single_goal(start, goal_d3, false, true),
        GoalClassification::Unreachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, goal_c4, false, true),
        GoalClassification::Unreachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, goal_e6, false, true),
        GoalClassification::Unreachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, goal_f5, false, true),
        GoalClassification::Unreachable { .. }
    ));

    // 黒手番(X)に反転
    let swap_d3 = goal_d3.swapped();
    let swap_c4 = goal_c4.swapped();
    let swap_e6 = goal_e6.swapped();
    let swap_f5 = goal_f5.swapped();

    assert!(matches!(
        classify_single_goal(start, swap_d3, false, true),
        GoalClassification::Reachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, swap_c4, false, true),
        GoalClassification::Reachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, swap_e6, false, true),
        GoalClassification::Reachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, swap_f5, false, true),
        GoalClassification::Reachable { .. }
    ));
}

#[test]
fn test_turn_canonicalization_1() {
    let start = Board::initial();

    // H=1 では実際の手番は白なので、黒番正規化した goal は黒白が反転した形で入力される。
    let goal =
        parse_line_to_board("-------------------O-------OO------OX---------------------------")
            .unwrap();
    let goal_black_fixed =
        parse_line_to_board("-------------------X-------XX------XO---------------------------")
            .unwrap();

    assert!(matches!(
        classify_single_goal(start, goal, false, true),
        GoalClassification::Reachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, goal_black_fixed, false, true),
        GoalClassification::Unreachable { .. }
    ));
}

#[test]
fn test_turn_canonicalization_2() {
    let start =
        parse_line_to_board("OOOOOOOOXXXXOOOOOOOOOOXOOOOOXOXOOOXOXOXO-OXXOXOOOOOOOOOOOOOOOOOX")
            .unwrap();

    let goal_black_fixed =
        parse_line_to_board("XXXXXXXXOOOOXXXXOXXXXXOXOXXXOXOXOXOXOXOXOOOOXOXXXXXXXXXXXXXXXXXO")
            .unwrap();
    let goal_white_relative =
        parse_line_to_board("OOOOOOOOXXXXOOOOXOOOOOXOXOOOXOXOXOXOXOXOXXXXOXOOOOOOOOOOOOOOOOOX")
            .unwrap();
    assert!(goal_white_relative.swapped() == goal_black_fixed);

    assert!(matches!(
        classify_single_goal(start, goal_black_fixed, false, false),
        GoalClassification::Reachable { .. }
    ));
    assert!(matches!(
        classify_single_goal(start, goal_white_relative, false, false),
        GoalClassification::Unreachable { .. }
    ));
}

#[test]
fn test_search_result() {
    let max_discs = 6;
    let start = Board::initial();
    let reachable = enumerate_reachable_states(max_discs);
    for &(goal, _turn_black) in &reachable {
        if !(4..=max_discs).contains(&goal.popcount()) {
            continue;
        }
        assert!(
            matches!(
                classify_single_goal(start, goal, false, true),
                GoalClassification::Reachable { .. }
            ),
            "expected SAT for reachable goal {}",
            goal.to_string()
        );
    }
}
