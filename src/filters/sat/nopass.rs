//! Find a replay-verified witness without passes, using shared directional rays.
//!
//! The mover alternates from the requested starting turn. Each transition places
//! one disc, so the number of transitions is fixed by the endpoint occupancies.
//! A restricted UNSAT result cannot establish general unreachability: every CNF
//! produced here is marked `ProofScope::WitnessOnly`.

use crate::filters::sat::cnf::{fix_board, neighbor, value, Builder, FALSE, SQUARES};
use crate::filters::sat::{CnfProblem, EncodingResult, ModelDecoder, ProofScope};
use crate::othello::{opposite, validate_board, Board, Color, Direction, Position};
use crate::query::ExactOutcome;
use crate::record::{replay, PlyAction};

struct Variables {
    black: Vec<[i32; SQUARES]>,
    white: Vec<[i32; SQUARES]>,
    play: Vec<[i32; SQUARES]>,
}

fn turn_at(start: Color, layer: usize) -> Color {
    if layer.is_multiple_of(2) {
        start
    } else {
        start.opposite()
    }
}

fn encode_transition(
    builder: &mut Builder,
    vars: &Variables,
    layer: usize,
    mover: Color,
    target_occupied: u64,
) {
    let (own, opponent, next_own, next_opponent) = match mover {
        Color::Black => (
            &vars.black[layer],
            &vars.white[layer],
            &vars.black[layer + 1],
            &vars.white[layer + 1],
        ),
        Color::White => (
            &vars.white[layer],
            &vars.black[layer],
            &vars.white[layer + 1],
            &vars.black[layer + 1],
        ),
    };
    let moves = &vars.play[layer];
    let directions = Direction::all();
    let mut rays = [[FALSE; SQUARES]; 8];
    let mut directional_flips = [[FALSE; SQUARES]; 8];

    for (index, &direction) in directions.iter().enumerate() {
        for square in 0..SQUARES {
            if target_occupied & (1_u64 << square) == 0 {
                continue;
            }
            if neighbor(square, direction)
                .is_some_and(|next| target_occupied & (1_u64 << next) != 0)
            {
                rays[index][square] = builder.variable();
                if neighbor(square, opposite(direction)).is_some() {
                    directional_flips[index][square] = builder.variable();
                }
            }
        }
    }

    for (index, &direction) in directions.iter().enumerate() {
        for square in 0..SQUARES {
            let ray = rays[index][square];
            if ray == FALSE {
                continue;
            }
            let next = neighbor(square, direction).expect("a ray has a neighbor");
            // Starting at square: one or more opponent discs, then our disc.
            builder.equivalent_and_or(ray, opponent[square], own[next], rays[index][next]);
            if let Some(previous) = neighbor(square, opposite(direction)) {
                // A bracketed line flips when reached from the placement or an
                // already flipping predecessor on the same directed line.
                builder.equivalent_and_or(
                    directional_flips[index][square],
                    ray,
                    moves[previous],
                    directional_flips[index][previous],
                );
            }
        }
    }

    for square in 0..SQUARES {
        let placement = moves[square];
        if placement != FALSE {
            builder.clause(&[-placement, -own[square]]);
            builder.clause(&[-placement, -opponent[square]]);
            let mut legal = vec![-placement];
            for (index, &direction) in directions.iter().enumerate() {
                if let Some(next) = neighbor(square, direction) {
                    legal.push(rays[index][next]);
                }
            }
            builder.clause(&legal);
        }
        if target_occupied & (1_u64 << square) == 0 {
            continue;
        }
        let sources: Vec<_> = directional_flips.iter().map(|line| line[square]).collect();
        let flipped = builder.disjunction(&sources);
        builder.equivalent_or(next_own[square], &[own[square], placement, flipped]);
        // Opponent discs remain exactly when they were present and did not flip.
        builder.clause(&[-next_opponent[square], opponent[square]]);
        builder.clause(&[-next_opponent[square], -flipped]);
        builder.clause(&[next_opponent[square], -opponent[square], flipped]);
    }
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
        let mut actions = Vec::with_capacity(self.vars.play.len());
        for (layer, moves) in self.vars.play.iter().enumerate() {
            let mut chosen = None;
            for (square, &placement) in moves.iter().enumerate() {
                if value(solver, placement)? && chosen.replace(square).is_some() {
                    return Err(format!("multiple placements at no-pass SAT layer {layer}"));
                }
            }
            let square =
                chosen.ok_or_else(|| format!("missing placement at no-pass SAT layer {layer}"))?;
            actions.push(PlyAction::Play(square));
        }
        let trace = replay(self.start, &actions)?;
        if trace.len() != self.vars.black.len() || trace.last() != Some(&self.goal) {
            return Err("no-pass SAT witness does not reach the exact requested goal".to_owned());
        }
        for (layer, actual) in trace.iter().enumerate() {
            let (mut black, mut white) = (0, 0);
            for square in 0..SQUARES {
                if value(solver, self.vars.black[layer][square])? {
                    black |= 1_u64 << square;
                }
                if value(solver, self.vars.white[layer][square])? {
                    white |= 1_u64 << square;
                }
            }
            let side_to_move = turn_at(self.start.side_to_move, layer);
            let expected = Position {
                board: match side_to_move {
                    Color::Black => Board::new(black, white),
                    Color::White => Board::new(white, black),
                },
                side_to_move,
            };
            if actual != &expected {
                return Err(format!(
                    "no-pass SAT model disagrees with replay at layer {layer}"
                ));
            }
        }
        Ok(actions)
    }
}

pub(crate) fn encode(start: Position, goal: Position) -> Result<EncodingResult, String> {
    validate_board(&start.board).map_err(|error| format!("invalid start board: {error:?}"))?;
    validate_board(&goal.board).map_err(|error| format!("invalid goal board: {error:?}"))?;
    if start == goal {
        return Ok(EncodingResult::Trivial(ExactOutcome::Reachable(Vec::new())));
    }
    let start_occupied = start.board.player | start.board.opponent;
    let target_occupied = goal.board.player | goal.board.opponent;
    let Some(placements) = goal.board.popcount().checked_sub(start.board.popcount()) else {
        return Ok(EncodingResult::Trivial(ExactOutcome::Unknown));
    };
    let placements = placements as usize;
    if placements == 0
        || start_occupied & target_occupied != start_occupied
        || turn_at(start.side_to_move, placements) != goal.side_to_move
    {
        return Ok(EncodingResult::Trivial(ExactOutcome::Unknown));
    }

    let mut builder = Builder::new();
    let added = target_occupied & !start_occupied;
    let vars = Variables {
        black: (0..=placements)
            .map(|_| builder.board_layer(target_occupied))
            .collect(),
        white: (0..=placements)
            .map(|_| builder.board_layer(target_occupied))
            .collect(),
        play: (0..placements)
            .map(|_| builder.board_layer(added))
            .collect(),
    };
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
    for layer in 0..placements {
        encode_transition(
            &mut builder,
            &vars,
            layer,
            turn_at(start.side_to_move, layer),
            target_occupied,
        );
    }
    Ok(EncodingResult::Cnf(CnfProblem::new(
        builder.variables as usize,
        builder.clauses,
        Decoder { vars, start, goal },
        ProofScope::WitnessOnly,
    )))
}
