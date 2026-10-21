use clap::{Args, Parser, Subcommand, ValueEnum};
use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{validate_board, Board, Color, Position};
use othello_complexity_rs::sampling::{
    artifact, create_output_dir, is_representative, population, representative_candidates,
    select_representative, write_sampling_record,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde_json::json;
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Explicit conversion of relative-board data to absolute positions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Args)]
struct Input {
    #[arg(long)]
    input: PathBuf,
    /// A new directory for positions.txt, mapping.jsonl, and sampling.json
    #[arg(long)]
    out_dir: PathBuf,
}
#[derive(Args)]
struct Sample {
    #[command(flatten)]
    input: Input,
    #[arg(long)]
    seed: u64,
    /// Sampling design of the original uniform relative-board sample
    #[arg(long, value_enum)]
    source_sampling: SourceSampling,
    /// Audit identifying the original uniform sampling and representative selection
    #[arg(long)]
    provenance: PathBuf,
}
#[derive(Clone, Copy, ValueEnum)]
enum SourceSampling {
    WithReplacement,
    WithoutReplacement,
}
#[derive(Clone, Copy, ValueEnum)]
enum Turn {
    B,
    W,
}
#[derive(Subcommand)]
enum Command {
    /// Rejection-resample a uniform sample of (occupancy, player) minima
    Representatives(Sample),
    /// Assign a fair absolute turn to each unfiltered uniform relative board
    Raw(Sample),
    /// Supply a known experimental turn; this is not uniform population sampling
    FixedTurn {
        #[command(flatten)]
        input: Input,
        #[arg(long, value_enum)]
        turn: Turn,
    },
}

/// The 64-cell research format is accepted only by this explicit converter.
/// Whitespace, annotations, missing central cells, and ignored lines are forbidden.
fn read_relative_boards(path: &std::path::Path) -> Result<Vec<Board>, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    contents
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let fail = |message: &str| -> Box<dyn Error> {
                format!("line {}: {message}", index + 1).into()
            };
            if line.len() != 64 {
                return Err(fail("expected exactly 64 X/O/- cells"));
            }
            let mut board = Board::empty();
            for (square, cell) in line.bytes().enumerate() {
                match cell {
                    b'X' => board.player |= 1_u64 << square,
                    b'O' => board.opponent |= 1_u64 << square,
                    b'-' => (),
                    _ => return Err(fail("invalid board character")),
                }
            }
            validate_board(&board).map_err(|error| fail(&format!("invalid board: {error:?}")))?;
            Ok(board)
        })
        .collect()
}
fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let (input, sample, method, fixed_turn) = match &cli.command {
        Command::Representatives(sample) => (&sample.input, Some(sample), "representatives", None),
        Command::Raw(sample) => (&sample.input, Some(sample), "raw", None),
        Command::FixedTurn { input, turn } => (
            input,
            None,
            "fixed-turn",
            Some(match turn {
                Turn::B => Color::Black,
                Turn::W => Color::White,
            }),
        ),
    };
    let boards = read_relative_boards(&input.input)?;
    if method == "representatives" {
        for (index, &board) in boards.iter().enumerate() {
            if !is_representative(board) {
                return Err(format!(
                    "line {}: board is not an (occupancy, player) representative",
                    index + 1
                )
                .into());
            }
        }
    }
    let source = artifact(&input.input)?;
    let provenance = sample.map(|s| artifact(&s.provenance)).transpose()?;
    create_output_dir(&input.out_dir)?;
    let positions_path = input.out_dir.join("positions.txt");
    let mapping_path = input.out_dir.join("mapping.jsonl");
    let mut output = BufWriter::new(File::create_new(&positions_path)?);
    let mut mapping = BufWriter::new(File::create_new(&mapping_path)?);
    let mut rng = sample.map(|sample| ChaCha8Rng::seed_from_u64(sample.seed));
    let mut accepted_count = 0;
    for (index, &board) in boards.iter().enumerate() {
        let (candidate_count, draw, chosen, symmetry) = match (&cli.command, &mut rng) {
            (Command::Representatives(_), Some(rng)) => {
                let candidates = representative_candidates(board);
                let draw = rng.random_range(0..16);
                let selected = select_representative(&candidates, draw)?;
                (
                    candidates.len(),
                    Some(draw),
                    selected.map(|(_, position)| position),
                    selected.map(|(symmetry, _)| symmetry.as_str()),
                )
            }
            (Command::Raw(_), Some(rng)) => {
                let draw = rng.random_range(0..2);
                (
                    2,
                    Some(draw),
                    Some(Position {
                        board,
                        side_to_move: if draw == 0 {
                            Color::Black
                        } else {
                            Color::White
                        },
                    }),
                    Some("identity"),
                )
            }
            (Command::FixedTurn { .. }, None) => {
                let side_to_move = fixed_turn.ok_or("missing fixed turn")?;
                (
                    1,
                    None,
                    Some(Position {
                        board,
                        side_to_move,
                    }),
                    Some("identity"),
                )
            }
            _ => return Err("inconsistent conversion configuration".into()),
        };
        let output_line = if let Some(position) = chosen {
            accepted_count += 1;
            writeln!(output, "{}", format_position(position))?;
            Some(accepted_count)
        } else {
            None
        };
        let row = json!({
            "input_line": index + 1, "output_line": output_line,
            "source_board": board.to_string(), "accepted": chosen.is_some(),
            "candidate_count": candidate_count, "draw": draw, "symmetry": symmetry,
            "side_to_move": chosen.map(|position| if position.side_to_move == Color::Black { "b" } else { "w" }),
            "position": chosen.map(format_position),
        });
        serde_json::to_writer(&mut mapping, &row)?;
        writeln!(mapping)?;
    }
    output.flush()?;
    mapping.flush()?;
    write_sampling_record(
        &input.out_dir,
        json!({
            "method": method, "population": population(), "uniform": sample.is_some(),
            "with_replacement": sample.map(|s| matches!(s.source_sampling, SourceSampling::WithReplacement)),
            "seed": sample.map(|s| s.seed), "input": source, "provenance": provenance,
            "input_count": boards.len(), "accepted_count": accepted_count,
            "output": artifact(&positions_path)?, "mapping": artifact(&mapping_path)?,
            "candidate_order": "BoardSymmetry::ALL, then Black/White; retain first distinct Position",
        }),
    )?;
    eprintln!("accepted {accepted_count} of {} input rows", boards.len());
    Ok(())
}
