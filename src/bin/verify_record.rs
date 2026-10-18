use clap::Parser;
use othello_complexity_rs::io::{format_position, parse_position};
use othello_complexity_rs::othello::{BoardSymmetry, Position};
use othello_complexity_rs::record::{parse_actions, replay, GoalMatch, ReachabilityRecord};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "verify-record",
    about = "Verify Othello reachability records with explicit turns and passes"
)]
struct Cli {
    /// Use the standard initial position with Black to move
    #[arg(long, conflicts_with = "start")]
    from_initial: bool,

    /// Start position: 64 X/O/- cells followed by a space and b or w (X = side to move)
    #[arg(long, value_name = "POSITION", allow_hyphen_values = true)]
    start: Option<String>,

    /// Goal position in the same format as --start
    #[arg(long, value_name = "POSITION", allow_hyphen_values = true)]
    goal: Option<String>,

    /// Space-separated moves and explicit ps tokens; an empty string is a zero-length record
    #[arg(long, value_name = "RECORD")]
    record: Option<String>,

    /// Six-column TSV records emitted by layer_sat
    #[arg(long, value_name = "FILE", conflicts_with_all = ["from_initial", "start", "goal", "record", "check_symmetry"])]
    input_file: Option<PathBuf>,

    /// Require the final position to match some rotation or reflection of the goal
    #[arg(long)]
    check_symmetry: bool,
}

fn main() {
    if let Err(err) = run(Cli::parse()) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    if let Some(path) = &cli.input_file {
        return run_file(path);
    }

    let start = match (cli.from_initial, cli.start.as_deref()) {
        (true, None) => Position::initial(),
        (false, Some(raw)) => parse_position(raw).map_err(|e| format!("invalid --start: {e}"))?,
        (true, Some(_)) => return Err("--from-initial and --start are mutually exclusive".into()),
        (false, None) => {
            return Err("start position is required: use --from-initial or --start".into())
        }
    };
    let goal = parse_position(
        cli.goal
            .as_deref()
            .ok_or("--goal is required without --input-file")?,
    )
    .map_err(|e| format!("invalid --goal: {e}"))?;
    let record = cli
        .record
        .as_deref()
        .ok_or("--record is required without --input-file")?;
    let goal_match = if cli.check_symmetry {
        GoalMatch::SymmetryOrbit
    } else {
        GoalMatch::Exact
    };
    let symmetry = verify_single(start, goal, record, goal_match)?;
    println!("OK: {} ({goal_match}, {symmetry})", format_position(goal));
    Ok(())
}

fn verify_single(
    start: Position,
    goal: Position,
    record: &str,
    goal_match: GoalMatch,
) -> Result<BoardSymmetry, String> {
    let actions = parse_actions(record)?;
    let positions = replay(start, &actions)?;
    let final_position = *positions
        .last()
        .ok_or("record replay returned no start position")?;
    let candidates = match goal_match {
        GoalMatch::Exact => &[BoardSymmetry::Identity][..],
        GoalMatch::SymmetryOrbit => &BoardSymmetry::ALL[..],
    };
    candidates
        .iter()
        .copied()
        .find(|symmetry| symmetry.apply_position(goal) == final_position)
        .ok_or_else(|| {
            format!(
                "record ends at {}, which does not match {} under {goal_match}",
                format_position(final_position),
                format_position(goal)
            )
        })
}

fn run_file(path: &Path) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let mut verified = 0;
    for (line_no, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| format!("{}: line {}: {e}", path.display(), line_no + 1))?;
        if line.is_empty() {
            continue;
        }
        // Keep the final tab: its empty field represents a zero-length record.
        let record = ReachabilityRecord::from_tsv(&line)
            .map_err(|e| format!("{}: line {}: {e}", path.display(), line_no + 1))?;
        record
            .verify()
            .map_err(|e| format!("{}: line {}: {e}", path.display(), line_no + 1))?;
        println!(
            "OK: input {} {} ({}, {})",
            record.input_index,
            format_position(record.goal),
            record.goal_match,
            record.symmetry
        );
        verified += 1;
    }
    println!("Verified {verified} record(s).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use othello_complexity_rs::othello::{Board, Color};

    #[test]
    fn file_input_cannot_be_overridden_by_single_record_arguments() {
        let position = format_position(Position::initial());
        for args in [
            vec!["--from-initial"],
            vec!["--start", &position],
            vec!["--goal", &position],
            vec!["--record", ""],
            vec!["--check-symmetry"],
        ] {
            let mut command = vec!["verify_record", "--input-file", "records.tsv"];
            command.extend(args);
            assert!(Cli::try_parse_from(command).is_err());
        }
        assert!(Cli::try_parse_from(["verify_record", "--input-file", "records.tsv"]).is_ok());
    }

    #[test]
    fn empty_record_preserves_the_absolute_turn() {
        let start = Position::initial();
        assert_eq!(
            verify_single(start, start, "", GoalMatch::Exact).unwrap(),
            BoardSymmetry::Identity
        );
        let different_turn = Position {
            side_to_move: Color::White,
            ..start
        };
        assert!(verify_single(start, different_turn, "", GoalMatch::Exact).is_err());
        assert!(verify_single(start, different_turn, "", GoalMatch::SymmetryOrbit).is_err());
    }

    #[test]
    fn symmetry_query_reports_a_transform_mapping_the_requested_goal_to_the_endpoint() {
        let start = Position::initial();
        let goal = BoardSymmetry::ALL
            .into_iter()
            .map(|s| s.apply_position(start))
            .find(|p| *p != start)
            .unwrap();
        assert!(verify_single(start, goal, "", GoalMatch::Exact).is_err());
        let symmetry = verify_single(start, goal, "", GoalMatch::SymmetryOrbit).unwrap();
        assert_eq!(symmetry.apply_position(goal), start);
    }

    #[test]
    fn a_legal_pass_must_be_explicit_and_change_the_absolute_turn() {
        let start =
            parse_position("OXX-OOOOOXXXXXXOOXOOOXXOOOXOXOXXOXOXOOXXOOOXXXOXOOXOOOXXOOOOOOXX w")
                .unwrap();
        let goal = Position {
            board: start.board.swapped(),
            side_to_move: Color::Black,
        };
        assert!(verify_single(start, goal, "ps", GoalMatch::Exact).is_ok());
        assert!(verify_single(start, goal, "", GoalMatch::Exact).is_err());
        assert!(verify_single(start, goal, "ps ps", GoalMatch::Exact).is_err());
        assert!(verify_single(
            start,
            Position {
                side_to_move: Color::White,
                ..goal
            },
            "ps",
            GoalMatch::Exact
        )
        .is_err());
    }

    #[test]
    fn terminal_positions_allow_empty_records_but_no_passes() {
        let start = Position {
            board: Board::new(u64::MAX, 0),
            side_to_move: Color::Black,
        };
        assert!(verify_single(start, start, "", GoalMatch::Exact).is_ok());
        assert!(verify_single(start, start, "ps", GoalMatch::SymmetryOrbit).is_err());
    }
}
