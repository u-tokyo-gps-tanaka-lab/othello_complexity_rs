use clap::{Parser, ValueEnum};
use othello_complexity_rs::io::{ensure_tri_outputs, parse_file_to_boards, parse_line_to_board};
use othello_complexity_rs::othello::Board;
use othello_complexity_rs::prunings::layer_sat::{run_with_options, RunOptions};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Side {
    Black,
    White,
}

impl Side {
    fn is_black(self) -> bool {
        matches!(self, Self::Black)
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "layer-sat",
    about = "Bounded SAT reachability checker for 8x8 Othello positions"
)]
struct Cli {
    /// Use the standard initial position as start board
    #[arg(long)]
    from_initial: bool,

    /// Start board as a 64-cell X/O/- string
    #[arg(long, value_name = "BOARD")]
    start: Option<String>,

    /// Goal board as a 64-cell X/O/- string
    #[arg(long, value_name = "BOARD")]
    goal: Option<String>,

    /// Input file containing one or more 64-cell X/O/- goal boards per line
    #[arg(long, value_name = "FILE")]
    goal_file: Option<PathBuf>,

    /// Starting side to move
    #[arg(long, value_enum, default_value_t = Side::Black)]
    start_turn: Side,

    /// Print coordinates for every layer
    #[arg(long, default_value_t = false)]
    show_coords: bool,

    /// Print X/O/- board strings for every layer
    #[arg(long, default_value_t = false)]
    show_boards: bool,

    /// Print encoding progress for each depth
    #[arg(long, default_value_t = false)]
    verbose: bool,

    /// Number of goal boards solved in parallel per Rayon batch
    #[arg(long, default_value_t = 1, value_parser = parse_positive_usize)]
    parallel_goals: usize,

    /// Output directory for layer SAT result files
    #[arg(
        short = 'o',
        long = "out-dir",
        value_name = "DIR",
        default_value = "result"
    )]
    out_dir: PathBuf,

    /// Emit DIMACS CNF files for each (goal, depth) instance
    #[arg(long, default_value_t = false)]
    dump_dimacs_cnf: bool,

    /// Generate DIMACS CNF files only (skip SAT solving)
    #[arg(long, default_value_t = false)]
    cnf_dump_only: bool,

    /// Per-depth SAT solver timeout in seconds (applied to each h)
    #[arg(long, value_name = "SECS", value_parser = parse_positive_u64)]
    sat_timeout_secs: Option<u64>,
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run_cli(cli) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run_cli(cli: Cli) -> Result<(), String> {
    let mut outputs = if cli.cnf_dump_only {
        None
    } else {
        Some(ensure_tri_outputs(&cli.out_dir, "layer_sat").map_err(|e| {
            format!(
                "failed to create output files in {}: {}",
                cli.out_dir.display(),
                e
            )
        })?)
    };

    let start = resolve_start_board(&cli)?;
    let goals = resolve_goal_boards(&cli)?;
    let cnf_dump_dir = resolve_cnf_dump_dir(&cli);
    run_with_options(
        RunOptions {
            start,
            goals,
            start_turn_black: cli.start_turn.is_black(),
            parallel_goals: cli.parallel_goals,
            show_coords: cli.show_coords,
            show_boards: cli.show_boards,
            verbose: cli.verbose,
            cnf_dump_dir,
            cnf_dump_only: cli.cnf_dump_only,
            sat_timeout_per_depth: cli.sat_timeout_secs.map(Duration::from_secs),
        },
        |outcome| {
            if cli.cnf_dump_only {
                return Ok(());
            }
            let outputs = outputs
                .as_mut()
                .expect("outputs must exist unless cnf_dump_only");
            outputs
                .write_result(outcome, &outcome.goal().to_string())
                .map_err(|e| format!("failed to write output: {e}"))?;
            outputs
                .flush()
                .map_err(|e| format!("failed to flush output files: {e}"))?;
            Ok(())
        },
    )?;

    if cli.cnf_dump_only {
        return Ok(());
    }
    Ok(())
}

fn resolve_cnf_dump_dir(cli: &Cli) -> Option<PathBuf> {
    if cli.dump_dimacs_cnf || cli.cnf_dump_only {
        return Some(cli.out_dir.clone());
    }
    None
}

fn resolve_start_board(cli: &Cli) -> Result<Board, String> {
    if cli.from_initial {
        if cli.start.is_some() {
            eprintln!("warning: --from-initial is set, so --start is ignored");
        }
        return Ok(Board::initial());
    }

    match cli.start.as_deref() {
        Some(raw) => parse_board_arg(raw, "--start"),
        None => Err("start board is required unless --from-initial is set (--start)".to_string()),
    }
}

fn resolve_goal_boards(cli: &Cli) -> Result<Vec<Board>, String> {
    if let Some(path) = &cli.goal_file {
        if cli.goal.is_some() {
            return Err("use either --goal-file or --goal, not both".to_string());
        }
        let path_text = path.to_string_lossy().to_string();
        let boards = parse_file_to_boards(&path_text)
            .map_err(|e| format!("failed to parse goal boards from {}: {}", path.display(), e))?;
        return Ok(boards);
    }

    match cli.goal.as_deref() {
        Some(raw) => Ok(vec![parse_board_arg(raw, "--goal")?]),
        None => Err("goal board is required: set --goal-file or --goal".to_string()),
    }
}

fn parse_board_arg(raw: &str, flag_name: &str) -> Result<Board, String> {
    parse_line_to_board(raw).ok_or_else(|| {
        format!(
            "invalid {} board: expected a 64-cell X/O/- string",
            flag_name
        )
    })
}

fn parse_positive_usize(raw: &str) -> Result<usize, String> {
    let value = raw
        .trim()
        .parse::<usize>()
        .map_err(|e| format!("invalid positive integer '{raw}': {e}"))?;
    if value == 0 {
        return Err("value must be >= 1".to_string());
    }
    Ok(value)
}

fn parse_positive_u64(raw: &str) -> Result<u64, String> {
    let value = raw
        .trim()
        .parse::<u64>()
        .map_err(|e| format!("invalid positive integer '{raw}': {e}"))?;
    if value == 0 {
        return Err("value must be >= 1".to_string());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_board_arg_accepts_x_o_dash_64_cells() {
        let board_str = Board::initial().to_string();
        assert_eq!(
            parse_board_arg(&board_str, "--start").unwrap(),
            Board::initial()
        );
    }

    #[test]
    fn parse_board_arg_rejects_short_input() {
        assert!(parse_board_arg("XOXO", "--goal").is_err());
    }

    #[test]
    fn parse_positive_usize_rejects_zero() {
        assert_eq!(parse_positive_usize("4").unwrap(), 4);
        assert!(parse_positive_usize("0").is_err());
    }

    #[test]
    fn parse_positive_u64_rejects_zero() {
        assert_eq!(parse_positive_u64("5").unwrap(), 5);
        assert!(parse_positive_u64("0").is_err());
    }
}
