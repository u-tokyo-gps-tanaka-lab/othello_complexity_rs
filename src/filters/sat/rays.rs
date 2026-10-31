//! Exact placement-layer SAT encoding with shared directional rays.
//!
//! Each transition places one disc. Forced passes before a placement and one
//! legal pass after the final placement are represented by the absolute color
//! of each mover. Directional capture conditions are shared across every
//! possible placement in a layer.

use crate::filters::sat::cnf::{fix_board, neighbor, value, Builder, FALSE, SQUARES};
use crate::filters::sat::placement::verify_replayed_layers;
use crate::filters::sat::{CnfProblem, EncodingResult, ModelDecoder, ProofScope};
use crate::othello::{get_moves, opposite, Board, Color, Direction, Position};
use crate::query::ExactOutcome;
use crate::record::{replay, PlyAction};

const COLORS: usize = 2;
const DIRECTIONS: usize = 8;
const BLACK: usize = 0;
const WHITE: usize = 1;

type BoardLayer = [i32; SQUARES];
type DirectionLayer = [[i32; SQUARES]; DIRECTIONS];
type RayLayer = [DirectionLayer; COLORS];

struct Variables {
    black: Vec<BoardLayer>,
    white: Vec<BoardLayer>,
    mover_black: Vec<i32>,
    play: Vec<BoardLayer>,
}

impl Variables {
    fn new(
        builder: &mut Builder,
        placements: usize,
        start_occupied: u64,
        target_occupied: u64,
    ) -> Self {
        let added = target_occupied & !start_occupied;
        Self {
            black: (0..=placements)
                .map(|_| builder.board_layer(target_occupied))
                .collect(),
            white: (0..=placements)
                .map(|_| builder.board_layer(target_occupied))
                .collect(),
            mover_black: (0..placements).map(|_| builder.variable()).collect(),
            play: (0..placements)
                .map(|_| builder.board_layer(added))
                .collect(),
        }
    }
}

fn encode_rays(
    builder: &mut Builder,
    vars: &Variables,
    placements: usize,
    target_occupied: u64,
) -> Vec<RayLayer> {
    let directions = Direction::all();
    let mut rays = Vec::with_capacity(placements);
    for _ in 0..placements {
        let mut layer = [[[FALSE; SQUARES]; DIRECTIONS]; COLORS];
        for color_rays in &mut layer {
            for (direction_index, &direction) in directions.iter().enumerate() {
                for (square, ray) in color_rays[direction_index].iter_mut().enumerate() {
                    if target_occupied & (1_u64 << square) != 0
                        && neighbor(square, direction)
                            .is_some_and(|next| target_occupied & (1_u64 << next) != 0)
                    {
                        *ray = builder.variable();
                    }
                }
            }
        }
        rays.push(layer);
    }

    for (layer, layer_rays) in rays.iter().enumerate() {
        for (color, color_rays) in layer_rays.iter().enumerate() {
            let (own, opponent) = if color == BLACK {
                (&vars.black[layer], &vars.white[layer])
            } else {
                (&vars.white[layer], &vars.black[layer])
            };
            for (direction_index, &direction) in directions.iter().enumerate() {
                for square in 0..SQUARES {
                    let ray = color_rays[direction_index][square];
                    if ray == FALSE {
                        continue;
                    }
                    let next = neighbor(square, direction).expect("a ray has a neighbor");
                    builder.equivalent_and_or(
                        ray,
                        opponent[square],
                        own[next],
                        color_rays[direction_index][next],
                    );
                }
            }
        }
    }
    rays
}

fn encode_transition(
    builder: &mut Builder,
    vars: &Variables,
    rays: &RayLayer,
    layer: usize,
    target_occupied: u64,
) {
    let directions = Direction::all();
    let mover_black = vars.mover_black[layer];
    let moves = &vars.play[layer];

    for (square, &placement) in moves.iter().enumerate() {
        if placement == FALSE {
            continue;
        }
        builder.clause(&[-placement, -vars.black[layer][square]]);
        builder.clause(&[-placement, -vars.white[layer][square]]);
        let mut legal_black = vec![-placement, -mover_black];
        let mut legal_white = vec![-placement, mover_black];
        for (direction_index, &direction) in directions.iter().enumerate() {
            if let Some(next) = neighbor(square, direction) {
                legal_black.push(rays[BLACK][direction_index][next]);
                legal_white.push(rays[WHITE][direction_index][next]);
            }
        }
        builder.clause(&legal_black);
        builder.clause(&legal_white);
    }

    let mut selected_rays = [[FALSE; SQUARES]; DIRECTIONS];
    let mut directional_flips = [[FALSE; SQUARES]; DIRECTIONS];
    for (direction_index, &direction) in directions.iter().enumerate() {
        for square in 0..SQUARES {
            let black_ray = rays[BLACK][direction_index][square];
            if black_ray == FALSE {
                continue;
            }
            let selected = builder.variable();
            builder.mux(
                selected,
                mover_black,
                black_ray,
                rays[WHITE][direction_index][square],
            );
            selected_rays[direction_index][square] = selected;
            if neighbor(square, opposite(direction)).is_some() {
                directional_flips[direction_index][square] = builder.variable();
            }
        }
    }

    for (direction_index, &direction) in directions.iter().enumerate() {
        for square in 0..SQUARES {
            let directional_flip = directional_flips[direction_index][square];
            if directional_flip == FALSE {
                continue;
            }
            let previous = neighbor(square, opposite(direction))
                .expect("a directional flip has a predecessor");
            builder.equivalent_and_or(
                directional_flip,
                selected_rays[direction_index][square],
                moves[previous],
                directional_flips[direction_index][previous],
            );
        }
    }

    for square in 0..SQUARES {
        if target_occupied & (1_u64 << square) == 0 {
            continue;
        }
        let flip_sources: Vec<_> = directional_flips
            .iter()
            .map(|direction| direction[square])
            .collect();
        let flipped = builder.disjunction(&flip_sources);
        let (black, white, next_black, next_white, placement) = (
            vars.black[layer][square],
            vars.white[layer][square],
            vars.black[layer + 1][square],
            vars.white[layer + 1][square],
            moves[square],
        );

        // Black mover: B' = B OR flip OR play, W' = W AND NOT flip.
        builder.clause(&[-mover_black, -black, next_black]);
        builder.clause(&[-mover_black, -flipped, next_black]);
        builder.clause(&[-mover_black, -placement, next_black]);
        builder.clause(&[-mover_black, black, flipped, placement, -next_black]);
        builder.clause(&[-mover_black, -white, flipped, next_white]);
        builder.clause(&[-mover_black, white, -next_white]);
        builder.clause(&[-mover_black, -flipped, -next_white]);

        // White mover: W' = W OR flip OR play, B' = B AND NOT flip.
        builder.clause(&[mover_black, -white, next_white]);
        builder.clause(&[mover_black, -flipped, next_white]);
        builder.clause(&[mover_black, -placement, next_white]);
        builder.clause(&[mover_black, white, flipped, placement, -next_white]);
        builder.clause(&[mover_black, -black, flipped, next_black]);
        builder.clause(&[mover_black, black, -next_black]);
        builder.clause(&[mover_black, -flipped, -next_black]);
    }
}

fn encode_turn_conditions(
    builder: &mut Builder,
    vars: &Variables,
    rays: &[RayLayer],
    start: Position,
    goal: Position,
    target_occupied: u64,
) {
    let first_mover = if get_moves(start.board.player, start.board.opponent) != 0 {
        start.side_to_move
    } else {
        start.side_to_move.opposite()
    };
    builder.clause(&[if first_mover == Color::Black {
        vars.mover_black[0]
    } else {
        -vars.mover_black[0]
    }]);

    let directions = Direction::all();
    for transition in 0..vars.mover_black.len().saturating_sub(1) {
        let layer = transition + 1;
        let current_mover = vars.mover_black[transition];
        let next_mover = vars.mover_black[transition + 1];
        for square in 0..SQUARES {
            for (direction_index, &direction) in directions.iter().enumerate() {
                let Some(next) = neighbor(square, direction) else {
                    continue;
                };
                if target_occupied & (1_u64 << next) == 0 {
                    continue;
                }
                // Consecutive Black placements require White to have no move.
                builder.clause(&[
                    -current_mover,
                    -next_mover,
                    vars.black[layer][square],
                    vars.white[layer][square],
                    -rays[layer][WHITE][direction_index][next],
                ]);
                // Consecutive White placements require Black to have no move.
                builder.clause(&[
                    current_mover,
                    next_mover,
                    vars.black[layer][square],
                    vars.white[layer][square],
                    -rays[layer][BLACK][direction_index][next],
                ]);
            }
        }
    }

    let pass_into_goal_is_legal = get_moves(goal.board.opponent, goal.board.player) == 0
        && get_moves(goal.board.player, goal.board.opponent) != 0;
    if !pass_into_goal_is_legal {
        let last_mover = vars.mover_black[vars.mover_black.len() - 1];
        builder.clause(&[if goal.side_to_move == Color::Black {
            -last_mover
        } else {
            last_mover
        }]);
    }
}

fn encode_problem(start: Position, goal: Position, placements: usize) -> (Variables, Builder) {
    let start_occupied = start.board.player | start.board.opponent;
    let target_occupied = goal.board.player | goal.board.opponent;
    let added = target_occupied & !start_occupied;
    let mut builder = Builder::new();
    let vars = Variables::new(&mut builder, placements, start_occupied, target_occupied);

    for layer in 0..=placements {
        for square in 0..SQUARES {
            builder.clause(&[-vars.black[layer][square], -vars.white[layer][square]]);
        }
    }
    fix_board(&mut builder, &vars.black[0], &vars.white[0], start);
    fix_board(
        &mut builder,
        &vars.black[placements],
        &vars.white[placements],
        goal,
    );

    for moves in &vars.play {
        builder.exactly_one(moves);
    }
    for square in 0..SQUARES {
        if added & (1_u64 << square) != 0 {
            let column: Vec<_> = vars.play.iter().map(|moves| moves[square]).collect();
            builder.exactly_one(&column);
        }
    }

    let rays = encode_rays(&mut builder, &vars, placements, target_occupied);
    for (layer, layer_rays) in rays.iter().enumerate() {
        encode_transition(&mut builder, &vars, layer_rays, layer, target_occupied);
    }
    encode_turn_conditions(&mut builder, &vars, &rays, start, goal, target_occupied);
    (vars, builder)
}

struct Decoder {
    vars: Variables,
    start: Position,
    goal: Position,
}

impl ModelDecoder for Decoder {
    fn decode_and_verify(
        &self,
        solver: &rustsat_kissat::Kissat<'_>,
    ) -> Result<Vec<PlyAction>, String> {
        let movers: Vec<_> = self
            .vars
            .mover_black
            .iter()
            .map(|&mover| {
                value(solver, mover).map(|black| if black { Color::Black } else { Color::White })
            })
            .collect::<Result<_, _>>()?;

        let mut layers = Vec::with_capacity(self.vars.black.len());
        for layer in 0..self.vars.black.len() {
            let mut black = 0_u64;
            let mut white = 0_u64;
            for square in 0..SQUARES {
                if value(solver, self.vars.black[layer][square])? {
                    black |= 1_u64 << square;
                }
                if value(solver, self.vars.white[layer][square])? {
                    white |= 1_u64 << square;
                }
            }
            let side_to_move = if layer == 0 {
                self.start.side_to_move
            } else {
                movers[layer - 1].opposite()
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

        let placements = self.vars.play.len();
        let mut actions = Vec::with_capacity(2 * placements + 1);
        let mut boundaries = Vec::with_capacity(placements + 1);
        boundaries.push(0);
        for layer in 0..placements {
            let pass_before = if layer == 0 {
                movers[layer] != self.start.side_to_move
            } else {
                movers[layer] == movers[layer - 1]
            };
            if pass_before {
                actions.push(PlyAction::Pass);
            }

            let mut chosen = None;
            for square in 0..SQUARES {
                if value(solver, self.vars.play[layer][square])? && chosen.replace(square).is_some()
                {
                    return Err(format!(
                        "multiple placements at placement-rays SAT layer {layer}"
                    ));
                }
            }
            let square = chosen
                .ok_or_else(|| format!("missing placement at placement-rays SAT layer {layer}"))?;
            actions.push(PlyAction::Play(square));
            boundaries.push(actions.len());
        }
        if movers.last() == Some(&self.goal.side_to_move) {
            actions.push(PlyAction::Pass);
        }

        let trace = replay(self.start, &actions)?;
        verify_replayed_layers(&layers, &trace, &boundaries)?;
        if trace.last() != Some(&self.goal) {
            return Err(
                "placement-rays SAT witness does not reach the exact requested goal".to_owned(),
            );
        }
        Ok(actions)
    }
}

pub(crate) fn encode(start: Position, goal: Position) -> Result<EncodingResult, String> {
    let start_occupied = start.board.player | start.board.opponent;
    let goal_occupied = goal.board.player | goal.board.opponent;
    let Some(placements) = goal.board.popcount().checked_sub(start.board.popcount()) else {
        return Ok(EncodingResult::Trivial(ExactOutcome::Unreachable));
    };
    if start_occupied & goal_occupied != start_occupied {
        return Ok(EncodingResult::Trivial(ExactOutcome::Unreachable));
    }
    if placements == 0 {
        return Err("placement-rays SAT encoding requires at least one placement".to_owned());
    }

    let (vars, builder) = encode_problem(start, goal, placements as usize);
    Ok(EncodingResult::Cnf(CnfProblem::new(
        builder.variables as usize,
        builder.clauses,
        Decoder { vars, start, goal },
        ProofScope::Complete,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::sat::cnf::TRUE;
    use crate::filters::sat::{placement, solve_problem};
    use crate::io::parse_position;

    fn solve_rays(start: Position, goal: Position) -> Result<ExactOutcome, String> {
        match encode(start, goal)? {
            EncodingResult::Trivial(outcome) => Ok(outcome),
            EncodingResult::Cnf(problem) => solve_problem(&problem, None),
        }
    }

    fn solve_placement(start: Position, goal: Position) -> Result<ExactOutcome, String> {
        match placement::encode(start, goal, true)? {
            EncodingResult::Trivial(outcome) => Ok(outcome),
            EncodingResult::Cnf(problem) => solve_problem(&problem, None),
        }
    }

    fn reachable(outcome: &ExactOutcome) -> bool {
        matches!(outcome, ExactOutcome::Reachable(_))
    }

    fn assert_agreement(start: Position, goal: Position) {
        let rays = solve_rays(start, goal).unwrap();
        let placement = solve_placement(start, goal).unwrap();
        assert_eq!(reachable(&rays), reachable(&placement));
    }

    fn leading_pass_start() -> Position {
        Position {
            board: Board::new(1_u64 << 1, !3_u64),
            side_to_move: Color::White,
        }
    }

    fn trailing_pass_start() -> Position {
        let empty = (1_u64 << 0) | (1_u64 << 8);
        let white = (1_u64 << 1) | (1_u64 << 9);
        Position {
            board: Board::new(!(empty | white), white),
            side_to_move: Color::Black,
        }
    }

    #[test]
    fn endpoint_shape_contract_distinguishes_unreachable_from_zero_placement() {
        let initial = Position::initial();
        let after_d3 = replay(initial, &[PlyAction::Play(19)]).unwrap()[1];
        let after_c4 = replay(initial, &[PlyAction::Play(26)]).unwrap()[1];
        for (start, goal) in [(after_d3, initial), (after_d3, after_c4)] {
            assert!(matches!(
                encode(start, goal).unwrap(),
                EncodingResult::Trivial(ExactOutcome::Unreachable)
            ));
        }
        assert!(encode(initial, initial).is_err());
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
        assert!(matches!(
            solve_rays(start, goal).unwrap(),
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
        let (vars, builder) = encode_problem(start, goal, placements);
        assert!(builder.variables > TRUE);
        assert!(!builder.clauses.is_empty());

        let goal_occupied = goal.board.player | goal.board.opponent;
        for square in 0..SQUARES {
            if goal_occupied & (1_u64 << square) != 0 {
                continue;
            }
            assert!(vars.black.iter().all(|layer| layer[square] == FALSE));
            assert!(vars.white.iter().all(|layer| layer[square] == FALSE));
            assert!(vars.play.iter().all(|layer| layer[square] == FALSE));
        }
    }

    #[test]
    fn three_move_history_is_reachable_and_replays() {
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
        let ExactOutcome::Reachable(actions) = solve_rays(start, goal).unwrap() else {
            panic!("placement-rays rejected a legal three-move history");
        };
        assert_eq!(replay(start, &actions).unwrap().last(), Some(&goal));
    }

    #[test]
    fn agrees_with_placement_on_openings_and_endpoint_passes() {
        let initial = Position::initial();
        for actions in [
            vec![PlyAction::Play(19)],
            vec![PlyAction::Play(19), PlyAction::Play(18)],
            vec![
                PlyAction::Play(19),
                PlyAction::Play(18),
                PlyAction::Play(17),
            ],
        ] {
            let goal = *replay(initial, &actions).unwrap().last().unwrap();
            assert_agreement(initial, goal);
        }

        let leading = leading_pass_start();
        let leading_goal = *replay(leading, &[PlyAction::Pass, PlyAction::Play(0)])
            .unwrap()
            .last()
            .unwrap();
        assert_agreement(leading, leading_goal);
        let ExactOutcome::Reachable(actions) = solve_rays(leading, leading_goal).unwrap() else {
            panic!("placement-rays rejected a legal leading pass");
        };
        assert_eq!(actions, [PlyAction::Pass, PlyAction::Play(0)]);

        let trailing = trailing_pass_start();
        let trailing_goal = *replay(trailing, &[PlyAction::Play(8), PlyAction::Pass])
            .unwrap()
            .last()
            .unwrap();
        assert_agreement(trailing, trailing_goal);
        let ExactOutcome::Reachable(actions) = solve_rays(trailing, trailing_goal).unwrap() else {
            panic!("placement-rays rejected a legal trailing pass");
        };
        assert_eq!(actions, [PlyAction::Play(8), PlyAction::Pass]);
    }

    #[test]
    fn mid_layer_pass_clauses_see_moves_outside_goal_for_both_colors() {
        for start in [
            Position::initial(),
            Position {
                board: Position::initial().board,
                side_to_move: Color::White,
            },
        ] {
            let first = replay(start, &[PlyAction::Play(19)]).unwrap()[1];
            let repeated_mover = Position {
                board: first.board.swapped(),
                side_to_move: start.side_to_move,
            };
            let goal = replay(repeated_mover, &[PlyAction::Play(45)]).unwrap()[1];
            let goal_occupied = goal.board.player | goal.board.opponent;
            assert_ne!(get_moves(first.board.player, first.board.opponent), 0);
            assert_eq!(
                get_moves(first.board.player, first.board.opponent) & goal_occupied,
                0
            );
            assert!(matches!(
                solve_placement(start, goal).unwrap(),
                ExactOutcome::Unreachable
            ));
            assert!(matches!(
                solve_rays(start, goal).unwrap(),
                ExactOutcome::Unreachable
            ));
        }
    }

    #[test]
    fn genuine_mid_game_forced_pass_is_reachable() {
        let start =
            parse_position("OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO w")
                .unwrap();
        let expected = [PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)];
        let goal = *replay(start, &expected).unwrap().last().unwrap();
        assert_agreement(start, goal);
        let ExactOutcome::Reachable(actions) = solve_rays(start, goal).unwrap() else {
            panic!("placement-rays rejected a legal middle pass");
        };
        assert_eq!(actions, expected);
        assert_eq!(replay(start, &actions).unwrap().last(), Some(&goal));
    }
}
