use clap::Parser;
use othello_complexity_rs::io::format_position;
use othello_complexity_rs::sampling::{
    artifact, create_output_dir, rank_of_position, write_sampling_record, Population, DEFAULT_SEED,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde_json::json;
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Sample Othello positions or equivalence classes uniformly with replacement")]
struct Args {
    /// Exact positions, or equivalence classes represented with Black to move
    #[arg(long, value_enum)]
    mode: Population,
    /// Total occupied squares; omit to sample the whole population
    #[arg(long, value_parser = clap::value_parser!(u32).range(4..=64))]
    discs: Option<u32>,
    #[arg(short = 'c', long)]
    count: usize,
    #[arg(long, default_value_t = DEFAULT_SEED)]
    seed: u64,
    /// New output directory
    #[arg(short = 'o', long)]
    out_dir: PathBuf,
}
fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    if args.count == 0 {
        return Err("count must be positive".into());
    }
    let population = args.mode.metadata(args.discs)?;
    create_output_dir(&args.out_dir)?;
    let positions_path = args.out_dir.join("positions.txt");
    let mapping_path = args.out_dir.join("mapping.jsonl");
    let mut output = BufWriter::new(File::create_new(&positions_path)?);
    let mut mapping = BufWriter::new(File::create_new(&mapping_path)?);
    let mut rng = ChaCha8Rng::seed_from_u64(args.seed);
    for index in 1..=args.count {
        let position = args.mode.sample(&mut rng, args.discs)?;
        writeln!(output, "{}", format_position(position))?;
        serde_json::to_writer(
            &mut mapping,
            &json!({
                "input_line": index, "output_line": index, "accepted": true,
                "position": format_position(position), "absolute_rank": rank_of_position(position)?.to_string(),
            }),
        )?;
        writeln!(mapping)?;
    }
    output.flush()?;
    mapping.flush()?;
    write_sampling_record(
        &args.out_dir,
        json!({
            "method": if args.discs.is_some() { "fixed-discs" } else { "uniform" },
            "population": population, "uniform": true, "with_replacement": true,
            "seed": args.seed, "input": null, "input_count": args.count,
            "accepted_count": args.count, "output": artifact(&positions_path)?,
            "mapping": artifact(&mapping_path)?,
        }),
    )?;
    Ok(())
}
