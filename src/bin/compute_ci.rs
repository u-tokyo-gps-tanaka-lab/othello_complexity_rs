use clap::Parser;
use std::error::Error;
use std::num::NonZeroU128;

use othello_complexity_rs::interval;

#[derive(Debug, Parser)]
#[command(
    name = "compute-ci",
    about = "Compute Wilson score confidence interval bounds"
)]
struct Args {
    /// Exact size of the population from which the sample was drawn
    #[arg(long)]
    population_size: NonZeroU128,

    /// Count of observed successes
    #[arg(long)]
    ok: u64,

    /// Count of observed failures
    #[arg(long)]
    ng: u64,

    /// Count of samples with unknown outcome
    #[arg(long)]
    unknown: u64,

    /// Significance level (two-sided alpha); e.g. 0.005 for 99.5% CI
    #[arg(long, default_value_t = 0.005)]
    alpha: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let interval = interval::compute(
        args.ok,
        args.ng,
        args.unknown,
        args.population_size,
        args.alpha,
    )?;
    println!("{interval}");
    Ok(())
}
