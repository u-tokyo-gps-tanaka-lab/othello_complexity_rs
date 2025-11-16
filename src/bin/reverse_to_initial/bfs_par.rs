use clap::Parser;

use othello_complexity_rs::search::bfs::Cfg;
use othello_complexity_rs::search::reverse_common::run_parallel_bfs;

fn main() {
    let cfg: Cfg = Cfg::parse();
    if let Err(e) = run_parallel_bfs(&cfg) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
