use std::path::Path;
use std::time::Duration;

use othello_complexity_rs::estimate::{Pipeline, SampleSpec, SatStage};
use othello_complexity_rs::filters::sat::{SatEncoding, SatPortfolio};
use othello_complexity_rs::sampling::Population;
use othello_complexity_rs::search::Strategy;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = Pipeline::sample(SampleSpec {
        population: Population::Classes,
        discs: None,
        sample_size: 1_000,
        seed: 20261022,
    })
    .filters()
    .reverse_search(Strategy::Bfs {
        discs: 10,
        max_nodes: 4_000_000,
        use_lp: false,
        threads: 1,
        parallel_goals: 20,
    })
    .sat(SatStage {
        encodings: SatPortfolio::new(vec![SatEncoding::NoPassWitness, SatEncoding::PlacementRays])?,
        seeds: 1,
        parallel_goals: 10,
        timeout: Some(Duration::from_secs(120)),
    })
    .reverse_search(Strategy::GbfsParallel {
        discs: 10,
        max_nodes: 10_000_000,
        use_lp: false,
        threads: 1,
        parallel_goals: 20,
    })
    .run(Path::new("out/estimate"), 0.005)?;
    println!("{}", report.interval);
    Ok(())
}
