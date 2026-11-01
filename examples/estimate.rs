use std::path::Path;
use std::time::Duration;

use othello_complexity_rs::estimate::{Pipeline, SampleSpec, SatStage};
use othello_complexity_rs::filters::sat::{ReverseBfs, SatEncoding};
use othello_complexity_rs::sampling::Population;
use othello_complexity_rs::search::run::Strategy;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = Pipeline::sample(SampleSpec {
        population: Population::Classes,
        discs: None,
        count: 1_000,
        seed: 20261022,
    })
    .filters()
    .sat(SatStage {
        encodings: SatEncoding::Ply.into(),
        seeds: 1,
        parallel_goals: 2,
        timeout: Some(Duration::from_secs(10)),
        reverse_bfs: Some(ReverseBfs {
            discs: 10,
            state_budget: 1_000_000,
        }),
    })
    .reverse_search(Strategy::DfsParallel {
        discs: 8,
        max_nodes: 10_000,
        table_size: 100_000,
        threads: 2,
    })
    .run(Path::new("out/estimate"), 0.005)?;
    println!("{}", report.interval);
    Ok(())
}
