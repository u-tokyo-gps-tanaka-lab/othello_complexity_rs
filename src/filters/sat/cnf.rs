use crate::othello::{Color, Direction, Position};
use rustsat::{
    solvers::Solve,
    types::{Lit, TernaryVal},
};

pub(super) const SQUARES: usize = 64;
// Variable 1 is fixed to true. Its two literals let target specialization omit
// variables entirely, rather than allocating and later fixing them to false.
pub(super) const TRUE: i32 = 1;
pub(super) const FALSE: i32 = -TRUE;

pub(super) struct Builder {
    pub(super) variables: i32,
    pub(super) clauses: Vec<Vec<i32>>,
}

impl Builder {
    pub(super) fn new() -> Self {
        Self {
            variables: TRUE,
            clauses: vec![vec![TRUE]],
        }
    }

    pub(super) fn variable(&mut self) -> i32 {
        self.variables += 1;
        self.variables
    }

    pub(super) fn clause(&mut self, literals: &[i32]) {
        if !literals.contains(&TRUE) {
            self.clauses.push(
                literals
                    .iter()
                    .copied()
                    .filter(|&literal| literal != FALSE)
                    .collect(),
            );
        }
    }

    pub(super) fn equivalent_or(&mut self, output: i32, inputs: &[i32]) {
        let mut clause = vec![-output];
        clause.extend_from_slice(inputs);
        self.clause(&clause);
        for &input in inputs {
            self.clause(&[output, -input]);
        }
    }

    pub(super) fn disjunction(&mut self, inputs: &[i32]) -> i32 {
        if inputs.contains(&TRUE) {
            return TRUE;
        }
        let inputs: Vec<_> = inputs
            .iter()
            .copied()
            .filter(|&literal| literal != FALSE)
            .collect();
        match inputs.as_slice() {
            [] => FALSE,
            [single] => *single,
            _ => {
                let output = self.variable();
                self.equivalent_or(output, &inputs);
                output
            }
        }
    }

    /// `output <-> first AND (second OR third)`.
    pub(super) fn equivalent_and_or(&mut self, output: i32, first: i32, second: i32, third: i32) {
        self.clause(&[-output, first]);
        self.clause(&[-output, second, third]);
        self.clause(&[output, -first, -second]);
        self.clause(&[output, -first, -third]);
    }

    pub(super) fn mux(&mut self, output: i32, condition: i32, yes: i32, no: i32) {
        self.clause(&[-condition, -yes, output]);
        self.clause(&[-condition, yes, -output]);
        self.clause(&[condition, -no, output]);
        self.clause(&[condition, no, -output]);
    }

    pub(super) fn exactly_one(&mut self, inputs: &[i32]) {
        let inputs: Vec<_> = inputs
            .iter()
            .copied()
            .filter(|&literal| literal != FALSE)
            .collect();
        self.clause(&inputs);
        let mut prefix = FALSE;
        for (index, &input) in inputs.iter().enumerate() {
            self.clause(&[-prefix, -input]);
            if index + 1 < inputs.len() {
                prefix = self.disjunction(&[prefix, input]);
            }
        }
    }

    pub(super) fn board_layer(&mut self, occupied: u64) -> [i32; SQUARES] {
        std::array::from_fn(|square| {
            if occupied & (1_u64 << square) != 0 {
                self.variable()
            } else {
                FALSE
            }
        })
    }
}

pub(super) fn lit(raw: i32) -> Result<Lit, String> {
    if raw > 0 {
        Ok(Lit::positive(raw as u32))
    } else if raw < 0 {
        Ok(Lit::negative((-raw) as u32))
    } else {
        Err("CNF contains literal 0".to_owned())
    }
}

pub(super) fn value(solver: &rustsat_kissat::Kissat<'_>, literal: i32) -> Result<bool, String> {
    if literal == TRUE {
        return Ok(true);
    }
    if literal == FALSE {
        return Ok(false);
    }
    match solver
        .lit_val(lit(literal)?)
        .map_err(|error| format!("failed to read SAT model literal {literal}: {error}"))?
    {
        TernaryVal::True => Ok(true),
        TernaryVal::False => Ok(false),
        TernaryVal::DontCare => Err(format!("SAT model literal {literal} has no value")),
    }
}

pub(super) fn absolute_colors(position: Position) -> (u64, u64) {
    match position.side_to_move {
        Color::Black => (position.board.player, position.board.opponent),
        Color::White => (position.board.opponent, position.board.player),
    }
}

pub(super) fn neighbor(square: usize, direction: Direction) -> Option<usize> {
    let (dx, dy) = direction.to_offset();
    let x = (square % 8) as i32 + dx;
    let y = (square / 8) as i32 + dy;
    if (0..8).contains(&x) && (0..8).contains(&y) {
        Some((8 * y + x) as usize)
    } else {
        None
    }
}

pub(super) fn fix_board(
    builder: &mut Builder,
    black: &[i32; SQUARES],
    white: &[i32; SQUARES],
    position: Position,
) {
    let (black_discs, white_discs) = absolute_colors(position);
    for square in 0..SQUARES {
        let bit = 1_u64 << square;
        builder.clause(&[if black_discs & bit != 0 {
            black[square]
        } else {
            -black[square]
        }]);
        builder.clause(&[if white_discs & bit != 0 {
            white[square]
        } else {
            -white[square]
        }]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustsat::{
        solvers::{Solve, SolverResult},
        types::Clause,
    };

    #[test]
    fn literals_use_raw_variable_indices() {
        assert_eq!(lit(1), Ok(Lit::positive(1)));
        assert_eq!(lit(-1), Ok(Lit::negative(1)));
        assert_eq!(lit(5), Ok(Lit::positive(5)));
        assert_eq!(lit(-5), Ok(Lit::negative(5)));
        assert!(lit(0).is_err());
    }

    #[test]
    fn values_include_constants_and_solver_assignments() {
        let mut solver = rustsat_kissat::Kissat::default();
        assert_eq!(value(&solver, TRUE), Ok(true));
        assert_eq!(value(&solver, FALSE), Ok(false));
        for raw in [TRUE, 2, -3] {
            solver
                .add_clause(Clause::from([lit(raw).unwrap()]))
                .unwrap();
        }
        assert_eq!(solver.solve().unwrap(), SolverResult::Sat);
        assert_eq!(value(&solver, 2), Ok(true));
        assert_eq!(value(&solver, -2), Ok(false));
        assert_eq!(value(&solver, 3), Ok(false));
    }
}
