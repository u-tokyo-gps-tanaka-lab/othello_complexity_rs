//! Samples 1,000,000 equivalence classes with seed 20261029 to estimate a 99.5% interval. Run it from the repository root with `cargo run --release -p experiments --bin 20261029-1m`.

use std::path::Path;
use std::time::Duration;

use othello_complexity_rs::estimate::{Pipeline, SampleSpec, SatStage};
use othello_complexity_rs::filters::sat::{ReverseBfs, SatEncoding, SatPortfolio};
use othello_complexity_rs::sampling::Population;
use othello_complexity_rs::search::run::Strategy;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = Pipeline::sample(SampleSpec {
        population: Population::Classes,
        discs: None,
        count: 1_000_000,
        seed: 20261029,
    })
    .filters()
    .sat(SatStage {
        encodings: SatPortfolio::new(vec![SatEncoding::NoPassWitness, SatEncoding::PlacementRays])?,
        seeds: 1,
        parallel_goals: 10,
        timeout: Some(Duration::from_secs(120)),
        reverse_bfs: Some(ReverseBfs {
            discs: 10,
            state_budget: 1_000_000,
        }),
    })
    .reverse_search(Strategy::GbfsParallel {
        discs: 10,
        max_nodes: 10_000_000,
        use_lp: false,
        threads: 20,
    })
    .run(Path::new("result/20261029-1m"), 0.005)?;
    println!("{}", report.interval);
    Ok(())
}
