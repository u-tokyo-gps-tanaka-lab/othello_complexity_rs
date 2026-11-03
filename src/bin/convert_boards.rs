use clap::{Args, Parser, Subcommand, ValueEnum};
use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{Color, Position};
use othello_complexity_rs::sampling::{
    artifact, create_output_dir, ensure_distinct_files, for_each_relative_board, is_representative,
    population, representative_candidates, select_representative, write_sampling_record,
    DEFAULT_SEED,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde_json::json;
use std::error::Error;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Explicit conversion of relative-board data to absolute positions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Args)]
struct Sample {
    #[arg(long)]
    input: PathBuf,
    /// A new directory for positions.txt, mapping.jsonl, and sampling.json
    #[arg(long)]
    out_dir: PathBuf,
    #[arg(long, default_value_t = DEFAULT_SEED)]
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
#[derive(Subcommand)]
enum Command {
    /// Rejection-resample a uniform sample of (occupancy, player) minima
    Representatives(Sample),
    /// Assign a fair absolute turn to each unfiltered uniform relative board
    Raw(Sample),
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let (sample, method) = match &cli.command {
        Command::Representatives(sample) => (sample, "representatives"),
        Command::Raw(sample) => (sample, "raw"),
    };
    let input_count = for_each_relative_board(&sample.input, |line_number, board| {
        if method == "representatives" && !is_representative(board) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line {line_number}: board is not an (occupancy, player) representative"),
            ));
        }
        Ok(())
    })?;
    let source = artifact(&sample.input)?;
    let provenance = artifact(&sample.provenance)?;
    let positions_path = sample.out_dir.join("positions.txt");
    let mapping_path = sample.out_dir.join("mapping.jsonl");
    let record_path = sample.out_dir.join("sampling.json");
    ensure_distinct_files(
        &sample.input,
        &[
            positions_path.as_path(),
            mapping_path.as_path(),
            record_path.as_path(),
        ],
    )?;
    create_output_dir(&sample.out_dir, "sampling.json")?;
    let mut output = BufWriter::new(File::create(&positions_path)?);
    let mut mapping = BufWriter::new(File::create(&mapping_path)?);
    let mut rng = ChaCha8Rng::seed_from_u64(sample.seed);
    let mut accepted_count = 0;
    let converted_count = for_each_relative_board(&sample.input, |line_number, board| {
        let (candidate_count, draw, chosen, symmetry) = match &cli.command {
            Command::Representatives(_) => {
                let candidates = representative_candidates(board);
                let draw = rng.random_range(0..16);
                let selected =
                    select_representative(&candidates, draw).map_err(io::Error::other)?;
                (
                    candidates.len(),
                    draw,
                    selected.map(|(_, position)| position),
                    selected.map(|(symmetry, _)| symmetry.as_str()),
                )
            }
            Command::Raw(_) => {
                let draw = rng.random_range(0..2);
                (
                    2,
                    draw,
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
        };
        let output_line = if let Some(position) = chosen {
            accepted_count += 1;
            writeln!(output, "{}", format_position(position))?;
            Some(accepted_count)
        } else {
            None
        };
        let row = json!({
            "input_line": line_number, "output_line": output_line,
            "source_board": board.to_string(), "accepted": chosen.is_some(),
            "candidate_count": candidate_count, "draw": draw, "symmetry": symmetry,
            "side_to_move": chosen.map(|position| if position.side_to_move == Color::Black { "b" } else { "w" }),
            "position": chosen.map(format_position),
        });
        serde_json::to_writer(&mut mapping, &row).map_err(io::Error::other)?;
        writeln!(mapping)?;
        Ok(())
    })?;
    if converted_count != input_count {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "input row count changed between validation and conversion: {input_count} to {converted_count}"
            ),
        )
        .into());
    }
    output.flush()?;
    mapping.flush()?;
    write_sampling_record(
        &sample.out_dir,
        json!({
            "method": method, "population": population(), "uniform": true,
            "with_replacement": matches!(sample.source_sampling, SourceSampling::WithReplacement),
            "seed": sample.seed, "input": source, "provenance": provenance,
            "input_count": input_count, "accepted_count": accepted_count,
            "output": artifact(&positions_path)?, "mapping": artifact(&mapping_path)?,
            "candidate_order": "BoardSymmetry::ALL, then Black/White; retain first distinct Position",
        }),
    )?;
    eprintln!("accepted {accepted_count} of {input_count} input rows");
    Ok(())
}
