use clap::Parser;
use othello_complexity_rs::sampling::{generate, Population, DEFAULT_SEED};
use std::error::Error;
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
    generate(&args.out_dir, args.mode, args.discs, args.count, args.seed)?;
    Ok(())
}
