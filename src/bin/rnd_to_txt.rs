use clap::Parser;
use othello_complexity_rs::io::format_position;
use othello_complexity_rs::sampling::{
    artifact, create_output_dir, population, position_from_rank, write_sampling_record,
};
use serde_json::json;
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Decode absolute-position ranks, one unsigned decimal integer per line")]
struct Args {
    input: PathBuf,
    #[arg(short = 'o', long)]
    out_dir: PathBuf,
}
fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let text = fs::read_to_string(&args.input)?;
    let positions = text
        .lines()
        .enumerate()
        .map(|(index, line)| {
            let result = if line.is_empty() || !line.bytes().all(|byte| byte.is_ascii_digit()) {
                Err("expected an unsigned decimal integer".to_owned())
            } else {
                line.parse::<u128>()
                    .map_err(|error| error.to_string())
                    .and_then(position_from_rank)
            };
            result.map_err(|error| format!("line {}: {error}", index + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let source = artifact(&args.input)?;
    create_output_dir(&args.out_dir)?;
    let positions_path = args.out_dir.join("positions.txt");
    let mut output = BufWriter::new(File::create_new(&positions_path)?);
    for &position in &positions {
        writeln!(output, "{}", format_position(position))?;
    }
    output.flush()?;
    write_sampling_record(
        &args.out_dir,
        json!({
            "method": "ranks", "population": population(), "uniform": false,
            "with_replacement": null, "seed": null, "input": source,
            "accepted_count": positions.len(), "output": artifact(&positions_path)?,
        }),
    )?;
    Ok(())
}
