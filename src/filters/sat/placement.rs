//! Exact reachability with one SAT transition per placed disc.
//!
//! A transition consists of either a legal placement by the current player, or
//! one forced pass followed by a legal placement by the other player. The last
//! layer may additionally take one legal pass. Absolute colors and the absolute
//! side to move are retained throughout. Every SAT witness is replayed using the
//! ordinary game rules, including its intermediate layer positions.

use crate::filters::sat::cnf::{absolute_colors, value, FALSE, SQUARES, TRUE};
use crate::filters::sat::{CnfProblem, EncodingResult, ModelDecoder, ProofScope};
use crate::othello::{get_moves, Board, Color, Direction, Position};
use crate::query::ExactOutcome;
use crate::record::{replay, PlyAction};

struct Builder {
    variables: i32,
    clauses: Vec<Vec<i32>>,
}

impl Builder {
    fn new() -> Self {
        Self {
            variables: TRUE,
            clauses: vec![vec![TRUE]],
        }
    }

    fn variable(&mut self) -> i32 {
        self.variables += 1;
        self.variables
    }

    fn clause(&mut self, mut literals: Vec<i32>) {
        if literals.contains(&TRUE) {
            return;
        }
        literals.retain(|&literal| literal != FALSE);
        // Constant folding is sufficient for most clauses. Sorting also avoids
        // duplicate literals or tautologies introduced by shared gate inputs.
        literals.sort_unstable();
        literals.dedup();
        if literals
            .iter()
            .any(|literal| literals.binary_search(&-literal).is_ok())
        {
            return;
        }
        self.clauses.push(literals);
    }

    fn unit(&mut self, literal: i32) {
        self.clause(vec![literal]);
    }

    fn equivalent(&mut self, output: i32, input: i32) {
        self.clause(vec![-output, input]);
        self.clause(vec![output, -input]);
    }

    fn conjunction(&mut self, inputs: &[i32]) -> i32 {
        if inputs.contains(&FALSE) {
            return FALSE;
        }
        let mut inputs: Vec<_> = inputs.iter().copied().filter(|&v| v != TRUE).collect();
        inputs.sort_unstable();
        inputs.dedup();
        if inputs
            .iter()
            .any(|literal| inputs.binary_search(&-literal).is_ok())
        {
            return FALSE;
        }
        match inputs.as_slice() {
            [] => TRUE,
            [single] => *single,
            _ => {
                let output = self.variable();
                for &input in &inputs {
                    self.clause(vec![-output, input]);
                }
                let mut clause: Vec<_> = inputs.iter().map(|&input| -input).collect();
                clause.push(output);
                self.clause(clause);
                output
            }
        }
    }

    fn disjunction(&mut self, inputs: &[i32]) -> i32 {
        // De Morgan also shares single inputs and folds constants.
        -self.conjunction(&inputs.iter().map(|&input| -input).collect::<Vec<_>>())
    }

    fn exactly_one(&mut self, inputs: &[i32]) {
        let inputs: Vec<_> = inputs.iter().copied().filter(|&v| v != FALSE).collect();
        self.clause(inputs.clone());
        let Some((&first, rest)) = inputs.split_first() else {
            return;
        };
        // A prefix-OR ladder gives linear, exact at-most-one constraints.
        let mut prefix = first;
        for (index, &input) in rest.iter().enumerate() {
            self.clause(vec![-prefix, -input]);
            if index + 1 < rest.len() {
                prefix = self.disjunction(&[prefix, input]);
            }
        }
    }

    fn xor(&mut self, output: i32, a: i32, b: i32) {
        self.clause(vec![a, b, -output]);
        self.clause(vec![-a, -b, -output]);
        self.clause(vec![a, -b, output]);
        self.clause(vec![-a, b, output]);
    }

    fn mux(&mut self, output: i32, condition: i32, yes: i32, no: i32) {
        self.clause(vec![-condition, -yes, output]);
        self.clause(vec![-condition, yes, -output]);
        self.clause(vec![condition, -no, output]);
        self.clause(vec![condition, no, -output]);
    }
}

/// A single directional capture pattern. The move square is deliberately
/// allowed to lie outside the target occupancy: such legal moves forbid passes
/// even though a successful history cannot select them as placements.
struct Pattern {
    square: usize,
    captured: Vec<usize>,
    anchor: usize,
}

fn patterns(start_occupied: u64, target_occupied: u64) -> Vec<Pattern> {
    let mut patterns = Vec::new();
    for square in 0..SQUARES {
        if start_occupied & (1_u64 << square) != 0 {
            // Initial occupied squares never become empty.
            continue;
        }
        for direction in Direction::all() {
            let (dx, dy) = direction.to_offset();
            let (mut x, mut y) = (square as i32 % 8 + dx, square as i32 / 8 + dy);
            let mut ray = Vec::new();
            while (0..8).contains(&x) && (0..8).contains(&y) {
                let next = (8 * y + x) as usize;
                if target_occupied & (1_u64 << next) == 0 {
                    // Every captured stone and the anchor must exist at the
                    // target. A permanent empty square blocks the whole ray.
                    break;
                }
                ray.push(next);
                x += dx;
                y += dy;
            }
            for length in 1..ray.len() {
                patterns.push(Pattern {
                    square,
                    captured: ray[..length].to_vec(),
                    anchor: ray[length],
                });
            }
        }
    }
    patterns
}

struct Variables {
    black: Vec<[i32; SQUARES]>,
    white: Vec<[i32; SQUARES]>,
    // The side to move immediately after the previous placement, before this
    // layer's optional pass. True means Black.
    turn_black: Vec<i32>,
    pass_before: Vec<i32>,
    play: Vec<[i32; SQUARES]>,
    pass_after: i32,
}

struct PlacementDecoder {
    vars: Variables,
    start: Position,
    goal: Position,
}

impl ModelDecoder for PlacementDecoder {
    fn decode_and_verify(
        &self,
        solver: &rustsat_kissat::Kissat<'_>,
    ) -> Result<Vec<PlyAction>, String> {
        decode_and_verify(solver, &self.vars, self.start, self.goal)
    }
}

impl Variables {
    fn new(
        builder: &mut Builder,
        placements: usize,
        start_occupied: u64,
        target_occupied: u64,
    ) -> Self {
        let play_mask = target_occupied & !start_occupied;
        let black = (0..=placements)
            .map(|_| {
                std::array::from_fn(|square| {
                    if target_occupied & (1_u64 << square) != 0 {
                        builder.variable()
                    } else {
                        FALSE
                    }
                })
            })
            .collect();
        let white = (0..=placements)
            .map(|_| {
                std::array::from_fn(|square| {
                    if target_occupied & (1_u64 << square) != 0 {
                        builder.variable()
                    } else {
                        FALSE
                    }
                })
            })
            .collect();
        let turn_black = (0..=placements).map(|_| builder.variable()).collect();
        let pass_before = (0..placements).map(|_| builder.variable()).collect();
        let play = (0..placements)
            .map(|_| {
                std::array::from_fn(|square| {
                    if play_mask & (1_u64 << square) != 0 {
                        builder.variable()
                    } else {
                        FALSE
                    }
                })
            })
            .collect();
        Self {
            black,
            white,
            turn_black,
            pass_before,
            play,
            pass_after: builder.variable(),
        }
    }
}

fn fixed_board(builder: &mut Builder, vars: &Variables, layer: usize, position: Position) {
    let (black, white) = absolute_colors(position);
    for square in 0..SQUARES {
        let bit = 1_u64 << square;
        builder.unit(if black & bit != 0 {
            vars.black[layer][square]
        } else {
            -vars.black[layer][square]
        });
        builder.unit(if white & bit != 0 {
            vars.white[layer][square]
        } else {
            -vars.white[layer][square]
        });
    }
}

fn encode_transition(builder: &mut Builder, vars: &Variables, layer: usize, patterns: &[Pattern]) {
    let current_black = &vars.black[layer];
    let current_white = &vars.white[layer];
    let turn_black = vars.turn_black[layer];
    // Every placement hands the move to the opposite absolute color.
    let mover_black = -vars.turn_black[layer + 1];
    let pass = vars.pass_before[layer];
    builder.xor(pass, turn_black, mover_black);
    builder.exactly_one(&vars.play[layer]);

    let mut captures_black: [Vec<i32>; SQUARES] = std::array::from_fn(|_| Vec::new());
    let mut captures_white: [Vec<i32>; SQUARES] = std::array::from_fn(|_| Vec::new());
    let mut flip_sources: [Vec<i32>; SQUARES] = std::array::from_fn(|_| Vec::new());
    for pattern in patterns {
        let square = pattern.square;
        let mut inputs_black = vec![-current_black[square], -current_white[square]];
        inputs_black.extend(pattern.captured.iter().map(|&sq| current_white[sq]));
        inputs_black.push(current_black[pattern.anchor]);
        let cap_black = builder.conjunction(&inputs_black);
        captures_black[square].push(cap_black);

        let mut inputs_white = vec![-current_black[square], -current_white[square]];
        inputs_white.extend(pattern.captured.iter().map(|&sq| current_black[sq]));
        inputs_white.push(current_white[pattern.anchor]);
        let cap_white = builder.conjunction(&inputs_white);
        captures_white[square].push(cap_white);

        let play = vars.play[layer][square];
        if play == FALSE {
            // Keep both legal-capture conditions above for forced-pass checks.
            continue;
        }
        // One selected capture is shared by every stone on its ray. It fires
        // iff this square is selected and the actual mover captures the ray.
        let fire = builder.variable();
        builder.clause(vec![-fire, play]);
        builder.clause(vec![-fire, -mover_black, cap_black]);
        builder.clause(vec![-fire, mover_black, cap_white]);
        builder.clause(vec![-play, -mover_black, -cap_black, fire]);
        builder.clause(vec![-play, mover_black, -cap_white, fire]);
        for &captured in &pattern.captured {
            flip_sources[captured].push(fire);
        }
    }

    let legal_black: [i32; SQUARES] =
        std::array::from_fn(|square| builder.disjunction(&captures_black[square]));
    let legal_white: [i32; SQUARES] =
        std::array::from_fn(|square| builder.disjunction(&captures_white[square]));
    let any_black = builder.disjunction(&legal_black);
    let any_white = builder.disjunction(&legal_white);
    // A pass is possible only when the current player has no legal move on
    // the entire board. The selected legal placement ensures the opponent can
    // move, so a terminal position cannot continue via a pass.
    builder.clause(vec![-pass, -turn_black, -any_black]);
    builder.clause(vec![-pass, turn_black, -any_white]);

    for square in 0..SQUARES {
        let play = vars.play[layer][square];
        builder.clause(vec![-play, -mover_black, legal_black[square]]);
        builder.clause(vec![-play, mover_black, legal_white[square]]);
        let flip = builder.disjunction(&flip_sources[square]);
        let changed = builder.disjunction(&[play, flip]);
        // A changed square becomes the mover's color; every other square keeps
        // its absolute color. This also preserves empty squares unless played.
        builder.mux(
            vars.black[layer + 1][square],
            changed,
            mover_black,
            current_black[square],
        );
        builder.mux(
            vars.white[layer + 1][square],
            changed,
            -mover_black,
            current_white[square],
        );
    }
}

fn encode_problem(
    start: Position,
    goal: Position,
    placements: usize,
    column_constraints: bool,
) -> (Variables, Builder) {
    let start_occupied = start.board.player | start.board.opponent;
    let target_occupied = goal.board.player | goal.board.opponent;
    let patterns = patterns(start_occupied, target_occupied);
    let mut builder = Builder::new();
    let vars = Variables::new(&mut builder, placements, start_occupied, target_occupied);
    for layer in 0..=placements {
        for square in 0..SQUARES {
            builder.clause(vec![-vars.black[layer][square], -vars.white[layer][square]]);
        }
    }
    fixed_board(&mut builder, &vars, 0, start);
    fixed_board(&mut builder, &vars, placements, goal);
    builder.unit(if start.side_to_move == Color::Black {
        vars.turn_black[0]
    } else {
        -vars.turn_black[0]
    });
    for layer in 0..placements {
        encode_transition(&mut builder, &vars, layer, &patterns);
    }
    if column_constraints {
        // Occupancy is monotone: each square added between the endpoints must
        // be played exactly once. This is already entailed by the transitions
        // and the fixed boards, but exposing it avoids deriving it through the
        // entire chain of color/flip gates. Together with one play per layer,
        // these columns describe a permutation of the newly occupied squares.
        let mut added = target_occupied & !start_occupied;
        while added != 0 {
            let square = added.trailing_zeros() as usize;
            added &= added - 1;
            let column: Vec<_> = vars.play.iter().map(|plays| plays[square]).collect();
            builder.exactly_one(&column);
        }
    }

    // The board at the final layer is fixed, so the legality of a pass into
    // the requested final turn can be computed exactly outside the SAT model.
    let pass_into_goal_is_legal = get_moves(goal.board.opponent, goal.board.player) == 0
        && get_moves(goal.board.player, goal.board.opponent) != 0;
    builder.equivalent(
        vars.pass_after,
        if goal.side_to_move == Color::Black {
            -vars.turn_black[placements]
        } else {
            vars.turn_black[placements]
        },
    );
    if !pass_into_goal_is_legal {
        builder.unit(-vars.pass_after);
    }
    (vars, builder)
}

fn decode_and_verify(
    solver: &rustsat_kissat::Kissat<'_>,
    vars: &Variables,
    start: Position,
    goal: Position,
) -> Result<Vec<PlyAction>, String> {
    let placements = vars.play.len();
    let mut layers = Vec::with_capacity(placements + 1);
    for layer in 0..=placements {
        let mut black = 0_u64;
        let mut white = 0_u64;
        for square in 0..SQUARES {
            if value(solver, vars.black[layer][square])? {
                black |= 1_u64 << square;
            }
            if value(solver, vars.white[layer][square])? {
                white |= 1_u64 << square;
            }
        }
        let side_to_move = if value(solver, vars.turn_black[layer])? {
            Color::Black
        } else {
            Color::White
        };
        layers.push(Position {
            board: if side_to_move == Color::Black {
                Board::new(black, white)
            } else {
                Board::new(white, black)
            },
            side_to_move,
        });
    }

    let mut actions = Vec::with_capacity(2 * placements + 1);
    let mut boundaries = Vec::with_capacity(placements + 1);
    boundaries.push(0);
    for layer in 0..placements {
        if value(solver, vars.pass_before[layer])? {
            actions.push(PlyAction::Pass);
        }
        let mut chosen = None;
        for square in 0..SQUARES {
            if value(solver, vars.play[layer][square])? && chosen.replace(square).is_some() {
                return Err(format!("multiple placements at SAT layer {layer}"));
            }
        }
        let square = chosen.ok_or_else(|| format!("missing placement at SAT layer {layer}"))?;
        actions.push(PlyAction::Play(square));
        boundaries.push(actions.len());
    }
    if value(solver, vars.pass_after)? {
        actions.push(PlyAction::Pass);
    }
    let trace = replay(start, &actions)?;
    verify_replayed_layers(&layers, &trace, &boundaries)?;
    if trace.last() != Some(&goal) {
        return Err("placement SAT witness does not reach the exact requested goal".to_owned());
    }
    Ok(actions)
}

fn verify_replayed_layers(
    layers: &[Position],
    trace: &[Position],
    boundaries: &[usize],
) -> Result<(), String> {
    for (layer, (&boundary, expected)) in boundaries.iter().zip(layers).enumerate() {
        if trace.get(boundary) != Some(expected) {
            return Err(format!(
                "placement SAT model disagrees with replay at layer {layer}"
            ));
        }
    }
    Ok(())
}

pub(crate) fn encode(
    start: Position,
    goal: Position,
    column_constraints: bool,
) -> Result<EncodingResult, String> {
    let start_occupied = start.board.player | start.board.opponent;
    let goal_occupied = goal.board.player | goal.board.opponent;
    let Some(placements) = goal.board.popcount().checked_sub(start.board.popcount()) else {
        return Ok(EncodingResult::Trivial(ExactOutcome::Unreachable));
    };
    if placements == 0 {
        return Err("placement SAT encoding requires at least one placement".to_owned());
    }
    if start_occupied & goal_occupied != start_occupied {
        return Ok(EncodingResult::Trivial(ExactOutcome::Unreachable));
    }
    let placements = placements as usize;
    let (vars, builder) = encode_problem(start, goal, placements, column_constraints);
    Ok(EncodingResult::Cnf(CnfProblem::new(
        builder.variables as usize,
        builder.clauses,
        PlacementDecoder { vars, start, goal },
        ProofScope::Complete,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::sat::solve_problem;

    struct Report {
        outcome: ExactOutcome,
        clauses: usize,
    }

    fn solve(start: Position, goal: Position) -> Result<Report, String> {
        solve_with_columns(start, goal, false)
    }

    fn solve_with_columns(
        start: Position,
        goal: Position,
        column_constraints: bool,
    ) -> Result<Report, String> {
        match encode(start, goal, column_constraints)? {
            EncodingResult::Trivial(outcome) => Ok(Report {
                clauses: usize::from(matches!(outcome, ExactOutcome::Unreachable)),
                outcome,
            }),
            EncodingResult::Cnf(problem) => {
                let clauses = problem.clause_count();
                Ok(Report {
                    outcome: solve_problem(&problem, None)?,
                    clauses,
                })
            }
        }
    }

    fn accepts(outcome: &ExactOutcome) -> bool {
        matches!(outcome, ExactOutcome::Reachable(_))
    }

    #[test]
    fn target_pruning_keeps_off_target_legal_moves_that_forbid_a_pass() {
        let start = Position::initial();
        let white_to_move = Position {
            board: start.board.swapped(),
            side_to_move: Color::White,
        };
        let goal = *replay(white_to_move, &[PlyAction::Play(20)])
            .unwrap()
            .last()
            .unwrap();
        // E3 is White's opening, while Black's legal openings all lie outside
        // this goal's occupancy. A false pass would incorrectly reach it.
        assert!(matches!(
            solve(start, goal).unwrap().outcome,
            ExactOutcome::Unreachable
        ));
    }

    #[test]
    fn goal_empty_squares_use_false_constants() {
        let start = Position::initial();
        let goal = *replay(start, &[PlyAction::Play(19)])
            .unwrap()
            .last()
            .unwrap();
        let placements = goal.board.popcount() as usize - start.board.popcount() as usize;
        assert!(placements > 0);
        let (vars, _) = encode_problem(start, goal, placements, false);
        let goal_occupied = goal.board.player | goal.board.opponent;
        for square in 0..SQUARES {
            if goal_occupied & (1_u64 << square) != 0 {
                continue;
            }
            assert!(
                vars.black.iter().all(|layer| layer[square] == FALSE),
                "black[{square}]"
            );
            assert!(
                vars.white.iter().all(|layer| layer[square] == FALSE),
                "white[{square}]"
            );
            assert!(
                vars.play.iter().all(|layer| layer[square] == FALSE),
                "play[{square}]"
            );
        }
    }

    #[test]
    fn placement_columns_preserve_a_legal_history_and_add_explicit_constraints() {
        let start = Position::initial();
        let goal = *replay(
            start,
            &[
                PlyAction::Play(19),
                PlyAction::Play(18),
                PlyAction::Play(17),
            ],
        )
        .unwrap()
        .last()
        .unwrap();
        let ordinary = solve(start, goal).unwrap();
        let with_columns = solve_with_columns(start, goal, true).unwrap();
        assert!(accepts(&ordinary.outcome));
        let ExactOutcome::Reachable(actions) = &with_columns.outcome else {
            panic!("placement columns rejected a legal history");
        };
        assert_eq!(replay(start, actions).unwrap().last(), Some(&goal));
        assert!(with_columns.clauses > ordinary.clauses);
    }

    #[test]
    fn replay_comparison_rejects_an_incorrect_intermediate_board() {
        let start = Position::initial();
        let trace = replay(start, &[PlyAction::Play(19), PlyAction::Play(18)]).unwrap();
        let mut layers = trace.clone();
        layers[1] = replay(start, &[PlyAction::Play(26)]).unwrap()[1];
        assert_eq!(layers.last(), trace.last());
        assert!(verify_replayed_layers(&layers, &trace, &[0, 1, 2]).is_err());
    }

    #[test]
    fn replay_comparison_rejects_an_incorrect_intermediate_absolute_turn() {
        let start = Position::initial();
        let trace = replay(start, &[PlyAction::Play(19), PlyAction::Play(18)]).unwrap();
        let mut layers = trace.clone();
        layers[1] = Position {
            board: layers[1].board.swapped(),
            side_to_move: layers[1].side_to_move.opposite(),
        };
        assert_eq!(layers.last(), trace.last());
        assert!(verify_replayed_layers(&layers, &trace, &[0, 1, 2]).is_err());
    }
}
