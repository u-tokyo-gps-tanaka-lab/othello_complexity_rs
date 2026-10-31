use crate::othello::{Color, Position};
use rustsat::{
    solvers::Solve,
    types::{Lit, TernaryVal},
};

pub(super) const SQUARES: usize = 64;
// Variable 1 is fixed to true. Its two literals let target specialization omit
// variables entirely, rather than allocating and later fixing them to false.
pub(super) const TRUE: i32 = 1;
pub(super) const FALSE: i32 = -TRUE;

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
