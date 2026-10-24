//! Ply-layer SAT encoding for exact reachability.

use crate::filters::sat::{CnfProblem, EncodingResult, ModelDecoder};
use crate::othello::{get_moves, Board, Position};
use crate::query::ExactOutcome;
use crate::record::{replay, PlyAction};
use rustsat::{
    solvers::Solve,
    types::{Lit, TernaryVal},
};

const BOARD_SQUARES: usize = 64;
const DIRS: [(i32, i32); 8] = [
    (0, 1),   // N
    (1, 1),   // NE
    (1, 0),   // E
    (1, -1),  // SE
    (0, -1),  // S
    (-1, -1), // SW
    (-1, 0),  // W
    (-1, 1),  // NW
];

#[derive(Clone, Copy, Debug)]
// 「play_sq に着手し、dir 方向で相手石を k 枚はさむ」を表す
struct FlipSource {
    play_sq: usize, // 着手マスの添字 (範囲は 0..BOARD_SQUARES)
    dir: usize,     // 方向添字 (範囲は 0..DIRS.len())
    k: usize,       // dir 方向で連続して挟む相手石の枚数 (範囲は 1..=rays[play_sq][dir].len()-1)
}

// rays[sq][dir]: sq から dir 方向へ進んだマスの一覧
// sq の範囲は 0..BOARD_SQUARES、dir の範囲は 0..DIRS.len()
type Rays = Vec<Vec<Vec<usize>>>;

// flip_sources[target_sq]: target_sq を反転させ得る FlipSource の一覧
// target_sq の範囲は 0..BOARD_SQUARES
type FlipSources = Vec<Vec<FlipSource>>;

#[derive(Default)]
struct ClauseBuilder {
    clauses: Vec<Vec<i32>>,
}

impl ClauseBuilder {
    fn add_clause(&mut self, clause: Vec<i32>) {
        self.clauses.push(clause);
    }

    fn add_unit(&mut self, lit: i32) {
        self.clauses.push(vec![lit]);
    }

    fn add_implication(&mut self, antecedent: i32, consequent: i32) {
        self.clauses.push(vec![-antecedent, consequent]);
    }

    fn add_implication_many(&mut self, antecedents: &[i32], consequent: i32) {
        let mut clause = Vec::with_capacity(antecedents.len() + 1);
        for &lit in antecedents {
            clause.push(-lit);
        }
        clause.push(consequent);
        self.clauses.push(clause);
    }

    // output <-> AND(inputs), including signed input literals.
    fn add_conjunction(&mut self, output: i32, inputs: &[i32]) {
        for &input in inputs {
            self.add_implication(output, input);
        }
        self.add_implication_many(inputs, output);
    }

    fn into_clauses(self) -> Vec<Vec<i32>> {
        self.clauses
    }
}

/// layer - 盤面の状態添字, 範囲は 0..=max_h（総数は max_h+1）
/// ply - 1手ごとの状態遷移の添字, 範囲は 0..max_h（総数は max_h）
struct SatVars {
    max_h: usize,                 // ありうる所要手数 (ply数) の最大値
    var_count: i32,               // これまでに割り当てた DIMACS 変数番号の最大値
    placement_mask: u64,          // actual placements/flips are allocated only here
    player: Vec<Vec<i32>>,        // player[layer][sq]: 手番側の石
    opponent: Vec<Vec<i32>>,      // opponent[layer][sq]: 相手側の石
    active: Vec<i32>,             // active[ply]: ply < h なら真
    end: Vec<i32>,                // end[layer]: layer = h なら真
    pass: Vec<i32>,               // pass[ply]: ply がパス手か否か
    play: Vec<Vec<i32>>,          // play[ply][sq]: ply で sq に着手する
    legal: Vec<Vec<i32>>,         // legal[ply][sq]: sq が ply の合法手である
    flip: Vec<Vec<i32>>,          // flip[ply][sq]: sq が ply で反転する
    cap: Vec<Vec<Vec<Vec<i32>>>>, // cap[ply][sq][dir][k]: sq への着手が方向 dir で k 枚を挟む
    cause: Vec<Vec<Vec<i32>>>,    // cause[ply][sq][i]: ply 手目で sq が反転する原因 i が成立する
    reply_cap: Vec<Vec<Vec<Vec<i32>>>>,
    // reply_cap[ply][sq][dir][k]: 現手番が ply で pass するとき、相手が sq に着手し dir 方向に k 枚挟める
}

impl SatVars {
    fn new(max_h: usize, rays: &Rays, flip_sources: &FlipSources, placement_mask: u64) -> Self {
        let mut next = 0_i32;

        let player = alloc_matrix(&mut next, max_h + 1, BOARD_SQUARES);
        let opponent = alloc_matrix(&mut next, max_h + 1, BOARD_SQUARES);
        let active = alloc_vector(&mut next, max_h);
        let end = alloc_vector(&mut next, max_h + 1);
        let pass = alloc_vector(&mut next, max_h);
        let play = alloc_masked_matrix(&mut next, max_h, placement_mask);
        let legal = alloc_matrix(&mut next, max_h, BOARD_SQUARES);
        let flip = alloc_masked_matrix(&mut next, max_h, placement_mask);
        let cap = alloc_cap_tensor(&mut next, max_h, rays);
        let reply_cap = alloc_cap_tensor(&mut next, max_h, rays);

        let mut cause: Vec<Vec<Vec<i32>>> = vec![vec![Vec::new(); BOARD_SQUARES]; max_h];
        for ply_causes in &mut cause {
            for (s, slot) in ply_causes.iter_mut().enumerate() {
                // One variable per precomputed source in flip_sources[s].
                let mut vars = vec![0_i32; flip_sources[s].len()];
                for var in &mut vars {
                    next += 1;
                    *var = next;
                }
                *slot = vars;
            }
        }

        Self {
            max_h,
            var_count: next,
            placement_mask,
            player,
            opponent,
            active,
            end,
            pass,
            play,
            legal,
            flip,
            cap,
            reply_cap,
            cause,
        }
    }
}

#[derive(Debug)]
struct SatWitness {
    actions: Vec<PlyAction>,
    boards: Vec<Board>,
}

struct LayerDecoder {
    vars: SatVars,
    start: Position,
    goal: Position,
}

impl ModelDecoder for LayerDecoder {
    fn decode_and_verify(
        &self,
        solver: &rustsat_kissat::Kissat<'_>,
    ) -> Result<Vec<PlyAction>, String> {
        let witness = decode_witness(&self.vars, solver)?;
        verify_witness(&witness, self.start, self.goal)?;
        Ok(witness.actions)
    }
}

pub(crate) fn encode(start: Position, goal: Position) -> Result<EncodingResult, String> {
    let Some(min_plies) = min_required_plies(start.board, goal.board) else {
        return Ok(EncodingResult::Trivial(ExactOutcome::Unreachable));
    };
    let max_plies = max_plies_with_passes(start.board, min_plies);
    if !(min_plies..=max_plies).any(|h| endpoint_turn_matches(start, goal, h)) {
        return Ok(EncodingResult::Trivial(ExactOutcome::Unreachable));
    }

    let mut rays = precompute_rays();
    let placement_mask = goal.board.player | goal.board.opponent;
    restrict_capture_rays(&mut rays, placement_mask);
    let mut flip_sources = precompute_flip_sources(&rays);
    for sources in &mut flip_sources {
        sources.retain(|source| bit_is_set(placement_mask, source.play_sq));
    }
    let vars = SatVars::new(max_plies, &rays, &flip_sources, placement_mask);
    let clauses = encode_problem(&vars, start, goal, min_plies, &rays, &flip_sources);
    Ok(EncodingResult::Cnf(CnfProblem::new(
        vars.var_count as usize,
        clauses,
        LayerDecoder { vars, start, goal },
    )))
}

fn min_required_plies(start: Board, goal: Board) -> Option<usize> {
    goal.popcount()
        .checked_sub(start.popcount())
        .map(|m| m as usize)
}

fn max_plies_with_passes(start: Board, placements: usize) -> usize {
    // Passes cannot be consecutive. Charge every pass except a possible first
    // one to its preceding placement. Thus passes <= placements + delta.
    let delta = usize::from(get_moves(start.player, start.opponent) == 0);
    2 * placements + delta
}

fn endpoint_turn_matches(start: Position, goal: Position, layer: usize) -> bool {
    layer.is_multiple_of(2) == (start.side_to_move == goal.side_to_move)
}

fn encode_problem(
    vars: &SatVars,
    start: Position,
    goal: Position,
    min_plies: usize,
    rays: &Rays,
    flip_sources: &FlipSources,
) -> Vec<Vec<i32>> {
    let mut builder = ClauseBuilder::default();
    add_range_selector_constraints(&mut builder, &vars.active, &vars.end, min_plies);

    // (1) Initial relative board and disjoint occupancy at every layer.
    for sq in 0..BOARD_SQUARES {
        builder.add_unit(if bit_is_set(start.board.player, sq) {
            vars.player[0][sq]
        } else {
            -vars.player[0][sq]
        });
        builder.add_unit(if bit_is_set(start.board.opponent, sq) {
            vars.opponent[0][sq]
        } else {
            -vars.opponent[0][sq]
        });
    }
    for layer in 0..=vars.max_h {
        for sq in 0..BOARD_SQUARES {
            if bit_is_set(vars.placement_mask, sq) {
                builder.add_clause(vec![-vars.player[layer][sq], -vars.opponent[layer][sq]]);
            } else {
                // Occupancy only grows. A square absent at the goal is empty
                // throughout every possible history. Inactive suffix layers
                // only exchange colors, so it is empty there as well.
                builder.add_unit(-vars.player[layer][sq]);
                builder.add_unit(-vars.opponent[layer][sq]);
            }
        }
        // (2) Only endpoints with the required absolute turn may be selected.
        if !endpoint_turn_matches(start, goal, layer) {
            builder.add_unit(-vars.end[layer]);
            continue;
        }
        for sq in 0..BOARD_SQUARES {
            if !bit_is_set(vars.placement_mask, sq) {
                continue;
            }
            builder.add_implication(
                vars.end[layer],
                if bit_is_set(goal.board.player, sq) {
                    vars.player[layer][sq]
                } else {
                    -vars.player[layer][sq]
                },
            );
            builder.add_implication(
                vars.end[layer],
                if bit_is_set(goal.board.opponent, sq) {
                    vars.opponent[layer][sq]
                } else {
                    -vars.opponent[layer][sq]
                },
            );
        }
    }

    for ply in 0..vars.max_h {
        let active = vars.active[ply];
        let pass = vars.pass[ply];

        // (3) Exactly one action when active; no action after the endpoint.
        let mut action_required = vec![-active, pass];
        action_required.extend(vars.play[ply].iter().copied().filter(|&play| play != 0));
        builder.add_clause(action_required);
        builder.add_implication(pass, active);
        for (sq, &play) in vars.play[ply].iter().enumerate() {
            if play == 0 {
                continue;
            }
            builder.add_implication(play, active);
            builder.add_clause(vec![-pass, -play]);
            for &earlier in vars.play[ply][..sq].iter().filter(|&&earlier| earlier != 0) {
                builder.add_clause(vec![-play, -earlier]);
            }
        }

        let mut pass_requires_reply = vec![-pass];
        // Keep legality at all 64 destinations, including outside the goal
        // occupancy: a forbidden actual placement can still make passing illegal.
        for (sq, square_rays) in rays.iter().enumerate() {
            let player = vars.player[ply][sq];
            let opponent = vars.opponent[ply][sq];
            let legal = vars.legal[ply][sq];
            let mut legal_requires_capture = vec![-legal];

            // (4) cap <-> active AND empty AND opponent run AND player anchor.
            // legal <-> OR(cap). These variables are false on inactive layers.
            for (dir, ray) in square_rays.iter().enumerate() {
                for k in 1..ray.len() {
                    let cap = vars.cap[ply][sq][dir][k];
                    let mut capture = vec![active, -player, -opponent];
                    capture.extend(ray[..k].iter().map(|&r| vars.opponent[ply][r]));
                    capture.push(vars.player[ply][ray[k]]);
                    builder.add_conjunction(cap, &capture);
                    builder.add_implication(cap, legal);
                    legal_requires_capture.push(cap);

                    // (5) The opponent must be able to move for a pass to be
                    // legal. Its capture conditions exchange player/opponent.
                    let reply_cap = vars.reply_cap[ply][sq][dir][k];
                    let mut reply = vec![active, -player, -opponent];
                    reply.extend(ray[..k].iter().map(|&r| vars.player[ply][r]));
                    reply.push(vars.opponent[ply][ray[k]]);
                    builder.add_conjunction(reply_cap, &reply);
                    pass_requires_reply.push(reply_cap);
                }
            }
            builder.add_clause(legal_requires_capture);
            builder.add_implication(pass, -legal);
            if vars.play[ply][sq] != 0 {
                builder.add_implication(vars.play[ply][sq], legal);
            }
        }
        builder.add_clause(pass_requires_reply);

        // (6) A square flips iff the chosen placement captures it in some ray.
        for (sq, sources) in flip_sources.iter().enumerate() {
            let flip = vars.flip[ply][sq];
            if flip == 0 {
                // Both colors are fixed false at every layer for this square.
                // No flip, cause or transition variables/clauses are needed.
                continue;
            }
            let mut flip_requires_cause = vec![-flip];
            for (source, &cause) in sources.iter().zip(&vars.cause[ply][sq]) {
                builder.add_conjunction(
                    cause,
                    &[
                        vars.play[ply][source.play_sq],
                        vars.cap[ply][source.play_sq][source.dir][source.k],
                    ],
                );
                builder.add_implication(cause, flip);
                flip_requires_cause.push(cause);
            }
            builder.add_clause(flip_requires_cause);
            builder.add_implication(flip, vars.opponent[ply][sq]);

            // (7) P' = O \ F; O' = P union F union M.
            // A pass and an inactive transition both have M=F=empty, hence
            // exchange relative viewpoints. Only active actions form a record.
            let next_player = vars.player[ply + 1][sq];
            let next_opponent = vars.opponent[ply + 1][sq];
            builder.add_conjunction(next_player, &[vars.opponent[ply][sq], -flip]);
            let additions = [vars.player[ply][sq], flip, vars.play[ply][sq]];
            for added in additions {
                builder.add_implication(added, next_opponent);
            }
            builder.add_clause(vec![
                -next_opponent,
                additions[0],
                additions[1],
                additions[2],
            ]);
        }
    }
    builder.into_clauses()
}

// h の範囲に関する制約を課す
//
// 1. ∀ ply < min_plies: active[ply]
// 2. ∀ ply < max_h - 1: active[ply + 1] -> active[ply]
// 3. end[0] <-> ¬active[0]
// 4. ∀ 1 <= layer < max_h:
//      end[layer] <-> (active[layer - 1] ∧ ¬active[layer])
// 5. end[max_h] <-> active[max_h - 1]
//
// 1により min_plies <= h が保証される。
// 2により active = (true, ..., true, false, ..., false) という形に限られる。
// 3-5 により、ある layer で end[layer] が真であることと h = layer は同値になる。
//
// ex1) h = 2, max_h = 5 のとき
//   active = [T, T, F, F, F]
//   end    = [F, F, T, F, F, F]
// ex2) h = max_h = 5 のとき
//   active = [T, T, T, T, T]
//   end    = [F, F, F, F, F, T]
fn add_range_selector_constraints(
    builder: &mut ClauseBuilder,
    active: &[i32],
    end: &[i32],
    min_plies: usize,
) {
    // max_h=0 -> end[0]
    if active.is_empty() {
        builder.add_unit(end[0]);
        return;
    }

    for (ply, &active_ply) in active.iter().enumerate() {
        if ply < min_plies {
            builder.add_unit(active_ply);
        }
        if ply + 1 < active.len() {
            builder.add_implication(active[ply + 1], active_ply);
        }
    }

    builder.add_clause(vec![-end[0], -active[0]]);
    builder.add_clause(vec![active[0], end[0]]);
    for layer in 1..active.len() {
        builder.add_implication(end[layer], active[layer - 1]);
        builder.add_clause(vec![-end[layer], -active[layer]]);
        builder.add_clause(vec![-active[layer - 1], active[layer], end[layer]]);
    }
    builder.add_implication(end[active.len()], active[active.len() - 1]);
    builder.add_implication(active[active.len() - 1], end[active.len()]);
}

fn verify_witness(witness: &SatWitness, start: Position, goal: Position) -> Result<(), String> {
    let positions = replay(start, &witness.actions)?;
    if positions.len() != witness.boards.len() {
        return Err("model verification failed: number of layers differs from replay".to_string());
    }
    for (layer, (position, board)) in positions.iter().zip(&witness.boards).enumerate() {
        if position.board != *board {
            return Err(format!(
                "model verification failed: layer {layer} differs from replay"
            ));
        }
    }
    if positions.last() != Some(&goal) {
        return Err(
            "model verification failed: endpoint position differs from requested goal".to_string(),
        );
    }
    Ok(())
}

fn decode_witness(
    vars: &SatVars,
    solver: &rustsat_kissat::Kissat<'_>,
) -> Result<SatWitness, String> {
    let h = decode_selected_h(vars, solver)?;
    let mut boards = Vec::with_capacity(h + 1);
    for layer in 0..=h {
        let mut player = 0_u64;
        let mut opponent = 0_u64;
        for sq in 0..BOARD_SQUARES {
            if value_of(solver, vars.player[layer][sq])? {
                player |= 1_u64 << sq;
            }
            if value_of(solver, vars.opponent[layer][sq])? {
                opponent |= 1_u64 << sq;
            }
        }
        boards.push(Board::new(player, opponent));
    }
    let mut actions = Vec::with_capacity(h);
    for ply in 0..h {
        let mut action = if value_of(solver, vars.pass[ply])? {
            Some(PlyAction::Pass)
        } else {
            None
        };
        for sq in 0..BOARD_SQUARES {
            if vars.play[ply][sq] != 0 && value_of(solver, vars.play[ply][sq])? {
                if action.is_some() {
                    return Err(format!(
                        "model decoding error: multiple actions at ply {ply}"
                    ));
                }
                action = Some(PlyAction::Play(sq));
            }
        }
        actions
            .push(action.ok_or_else(|| format!("model decoding error: no action at ply {ply}"))?);
    }
    Ok(SatWitness { actions, boards })
}

// Read the unique selected endpoint. Never infer missing model values.
fn decode_selected_h(vars: &SatVars, solver: &rustsat_kissat::Kissat<'_>) -> Result<usize, String> {
    let mut selected = None;
    for layer in 0..=vars.max_h {
        if value_of(solver, vars.end[layer])? {
            if selected.is_some() {
                return Err("model decoding error: multiple end layers selected".to_string());
            }
            selected = Some(layer);
        }
    }
    selected.ok_or_else(|| "model decoding error: no end layer selected".to_string())
}

fn value_of(solver: &rustsat_kissat::Kissat<'_>, var: i32) -> Result<bool, String> {
    let lit = Lit::positive(var as u32);
    match solver
        .lit_val(lit)
        .map_err(|err| format!("failed to read model literal {var}: {err}"))?
    {
        TernaryVal::True => Ok(true),
        TernaryVal::False => Ok(false),
        TernaryVal::DontCare => Err(format!("model decoding error: literal {var} has no value")),
    }
}

// Allocates `len` fresh variable ids as a 1D vector.
fn alloc_vector(next: &mut i32, len: usize) -> Vec<i32> {
    let mut out = vec![0_i32; len];
    for v in &mut out {
        *next += 1;
        *v = *next;
    }
    out
}

// Allocates `rows x cols` fresh variable ids as a 2D matrix.
fn alloc_matrix(next: &mut i32, rows: usize, cols: usize) -> Vec<Vec<i32>> {
    let mut out = vec![vec![0_i32; cols]; rows];
    for row in out.iter_mut().take(rows) {
        for cell in row.iter_mut().take(cols) {
            *next += 1;
            *cell = *next;
        }
    }
    out
}

/// Zero denotes an omitted variable, never a DIMACS literal. Callers must skip
/// these slots when encoding clauses or reading the model.
fn alloc_masked_matrix(next: &mut i32, rows: usize, mask: u64) -> Vec<Vec<i32>> {
    let mut out = vec![vec![0_i32; BOARD_SQUARES]; rows];
    for row in &mut out {
        for (sq, cell) in row.iter_mut().enumerate() {
            if bit_is_set(mask, sq) {
                *next += 1;
                *cell = *next;
            }
        }
    }
    out
}

fn alloc_cap_tensor(next: &mut i32, h: usize, rays: &Rays) -> Vec<Vec<Vec<Vec<i32>>>> {
    let mut cap = vec![vec![vec![Vec::new(); DIRS.len()]; BOARD_SQUARES]; h];
    for ply_caps in &mut cap {
        for (square_caps, square_rays) in ply_caps.iter_mut().zip(rays) {
            for (direction_caps, ray) in square_caps.iter_mut().zip(square_rays) {
                let max_k = ray.len().saturating_sub(1);
                // k = 0 is a dummy slot. Real capture variables are k >= 1 only.
                let mut ks = vec![0_i32; max_k + 1];
                for slot in ks.iter_mut().take(max_k + 1).skip(1) {
                    *next += 1;
                    *slot = *next;
                }
                *direction_caps = ks;
            }
        }
    }
    cap
}

// Precomputes ordered ray squares for every start square and direction.
fn precompute_rays() -> Rays {
    let mut rays = vec![vec![Vec::<usize>::new(); DIRS.len()]; BOARD_SQUARES];
    for (sq, sq_rays) in rays.iter_mut().enumerate().take(BOARD_SQUARES) {
        let x = (sq % 8) as i32;
        let y = (sq / 8) as i32;
        for (d, (dx, dy)) in DIRS.iter().enumerate() {
            let mut x1 = x + dx;
            let mut y1 = y + dy;
            while (0..8).contains(&x1) && (0..8).contains(&y1) {
                sq_rays[d].push((y1 * 8 + x1) as usize);
                x1 += dx;
                y1 += dy;
            }
        }
    }
    rays
}

/// A capture requires every intervening stone and its anchor to be occupied.
/// Once a ray leaves the final occupancy, no longer prefix can capture. The
/// starting square is deliberately unrestricted so hypothetical legal moves
/// outside the goal remain available to the forced-pass constraints.
fn restrict_capture_rays(rays: &mut Rays, occupied: u64) {
    for square_rays in rays {
        for ray in square_rays {
            let length = ray
                .iter()
                .position(|&sq| !bit_is_set(occupied, sq))
                .unwrap_or(ray.len());
            ray.truncate(length);
        }
    }
}

// Precomputes all capture patterns that can flip each target square.
fn precompute_flip_sources(rays: &Rays) -> FlipSources {
    let mut sources: FlipSources = vec![Vec::new(); BOARD_SQUARES];
    for (play_sq, play_rays) in rays.iter().enumerate().take(BOARD_SQUARES) {
        for (d, ray) in play_rays.iter().enumerate().take(DIRS.len()) {
            let max_k = ray.len().saturating_sub(1);
            for k in 1..=max_k {
                for target in ray.iter().take(k) {
                    sources[*target].push(FlipSource { play_sq, dir: d, k });
                }
            }
        }
    }
    sources
}

#[inline(always)]
fn bit_is_set(bb: u64, sq: usize) -> bool {
    (bb >> sq) & 1 == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::sat::solve_problem;
    use crate::io::{TriCategory, TriOutcome};
    use crate::othello::BoardSymmetry;
    use crate::query::{solve_query, GoalClassification};
    use crate::record::{GoalMatch, PositionTransform};
    use std::time::Duration;

    fn target_geometry(occupied: u64) -> (Rays, FlipSources) {
        let mut rays = precompute_rays();
        restrict_capture_rays(&mut rays, occupied);
        let mut sources = precompute_flip_sources(&rays);
        for square_sources in &mut sources {
            square_sources.retain(|source| bit_is_set(occupied, source.play_sq));
        }
        (rays, sources)
    }

    #[test]
    fn target_occupancy_pruning_reduces_cnf_without_zero_literals() {
        let start = Position::initial();
        let goal = *replay(start, &[PlyAction::Play(19), PlyAction::Play(18)])
            .unwrap()
            .last()
            .unwrap();
        let occupied = goal.board.player | goal.board.opponent;
        let min_plies = min_required_plies(start.board, goal.board).unwrap();
        let max_plies = max_plies_with_passes(start.board, min_plies);

        let original_rays = precompute_rays();
        let original_sources = precompute_flip_sources(&original_rays);
        let original_vars = SatVars::new(max_plies, &original_rays, &original_sources, u64::MAX);
        let original_cnf = encode_problem(
            &original_vars,
            start,
            goal,
            min_plies,
            &original_rays,
            &original_sources,
        );
        let (rays, sources) = target_geometry(occupied);
        let vars = SatVars::new(max_plies, &rays, &sources, occupied);
        let clauses = encode_problem(&vars, start, goal, min_plies, &rays, &sources);
        assert!(vars.var_count < original_vars.var_count);
        assert!(clauses.len() < original_cnf.len());
        for clause in &clauses {
            for &literal in clause {
                assert_ne!(literal, 0, "omitted variables must not become literals");
                assert!(literal.abs() <= vars.var_count);
            }
        }
    }

    #[test]
    fn target_occupancy_pruning_keeps_legality_outside_the_goal() {
        // If the only added square is D3, C4 is outside the goal. C4 is still
        // a legal initial Black move and must prevent an artificial pass.
        let occupied = crate::othello::CENTER_MASK | (1_u64 << 19);
        let (rays, sources) = target_geometry(occupied);
        let vars = SatVars::new(2, &rays, &sources, occupied);
        let c4 = 26;
        let east = 2;
        assert!(!bit_is_set(occupied, c4));
        assert_eq!(rays[c4][east], vec![27, 28]);
        assert_eq!(vars.play[0][c4], 0);
        assert_eq!(vars.flip[0][c4], 0);
        assert_ne!(vars.legal[0][c4], 0);
        assert_ne!(vars.cap[0][c4][east][1], 0);
        assert_ne!(vars.reply_cap[0][c4][east][1], 0);
        assert!(sources
            .iter()
            .flatten()
            .all(|source| bit_is_set(occupied, source.play_sq)));
    }

    #[test]
    fn target_occupancy_pruning_leaves_full_occupancy_cnf_unchanged() {
        let position = Position {
            board: Board::new(u64::MAX, 0),
            side_to_move: crate::othello::Color::Black,
        };
        let original_rays = precompute_rays();
        let original_sources = precompute_flip_sources(&original_rays);
        let original_vars = SatVars::new(1, &original_rays, &original_sources, u64::MAX);
        let (rays, sources) = target_geometry(u64::MAX);
        let vars = SatVars::new(1, &rays, &sources, u64::MAX);
        assert_eq!(vars.var_count, original_vars.var_count);
        assert_eq!(
            encode_problem(&vars, position, position, 0, &rays, &sources),
            encode_problem(
                &original_vars,
                position,
                position,
                0,
                &original_rays,
                &original_sources
            )
        );
    }

    // The orbit of the reflected initial position has two representatives
    // under the stabilizer of the fixed standard initial position.
    fn aggregate(
        mut results: Vec<Result<ExactOutcome, String>>,
    ) -> Result<Option<GoalClassification>, String> {
        results.reverse();
        solve_query(
            Position::initial(),
            BoardSymmetry::MirrorHorizontal.apply_position(Position::initial()),
            1,
            GoalMatch::SymmetryOrbit,
            |_, _| results.pop().expect("candidate count").map(Some),
        )
    }

    #[test]
    fn candidate_aggregation_preserves_unknown_and_errors() {
        let all_ng = aggregate(vec![
            Ok(ExactOutcome::Unreachable),
            Ok(ExactOutcome::Unreachable),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(all_ng.category(), TriCategory::Ng);
        for results in [
            vec![Ok(ExactOutcome::Unknown), Ok(ExactOutcome::Unreachable)],
            vec![Ok(ExactOutcome::Unreachable), Ok(ExactOutcome::Unknown)],
        ] {
            assert_eq!(
                aggregate(results).unwrap().unwrap().category(),
                TriCategory::Unknown
            );
        }
        assert!(aggregate(vec![
            Ok(ExactOutcome::Unknown),
            Err("solver failure".to_string())
        ])
        .is_err());
    }

    #[test]
    fn candidate_sat_after_unknown_is_verified_and_wins() {
        let outcome = aggregate(vec![
            Ok(ExactOutcome::Unknown),
            Ok(ExactOutcome::Reachable(vec![])),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(outcome.category(), TriCategory::Ok);
        let record = outcome.record().unwrap();
        assert_ne!(record.transform, PositionTransform::IDENTITY);
        assert_eq!(record.verify().unwrap(), vec![Position::initial()]);
    }

    #[test]
    fn corrupt_model_layers_and_endpoint_are_execution_errors() {
        let start = Position::initial();
        assert!(verify_witness(
            &SatWitness {
                actions: vec![],
                boards: vec![start.board.swapped()]
            },
            start,
            start
        )
        .is_err());
        let wrong_turn = Position {
            side_to_move: start.side_to_move.opposite(),
            ..start
        };
        assert!(verify_witness(
            &SatWitness {
                actions: vec![],
                boards: vec![start.board]
            },
            start,
            wrong_turn
        )
        .is_err());
        assert!(verify_witness(
            &SatWitness {
                actions: vec![PlyAction::Pass],
                boards: vec![start.board, start.board.swapped()],
            },
            start,
            start,
        )
        .is_err());
    }

    #[test]
    fn solver_timeout_is_unknown() {
        // Pigeonhole principle: 12 pigeons cannot occupy 11 holes with at most
        // one pigeon per hole. It is unsatisfiable independently of Othello.
        let mut clauses = Vec::new();
        let variable = |pigeon: i32, hole: i32| 1 + pigeon * 11 + hole;
        for pigeon in 0..12 {
            clauses.push((0..11).map(|hole| variable(pigeon, hole)).collect());
        }
        for hole in 0..11 {
            for pigeon in 0..12 {
                for earlier in 0..pigeon {
                    clauses.push(vec![-variable(pigeon, hole), -variable(earlier, hole)]);
                }
            }
        }
        let rays = precompute_rays();
        let vars = SatVars::new(0, &rays, &precompute_flip_sources(&rays), u64::MAX);
        let problem = CnfProblem::new(
            132,
            clauses,
            LayerDecoder {
                vars,
                start: Position::initial(),
                goal: Position::initial(),
            },
        );
        assert!(matches!(
            solve_problem(&problem, Some(Duration::from_millis(1))).unwrap(),
            ExactOutcome::Unknown
        ));
    }
}
