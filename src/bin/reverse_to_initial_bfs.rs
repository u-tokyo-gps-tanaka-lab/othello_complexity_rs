use clap::Parser;

use othello_complexity_rs::lib::bfs_search::Cfg;
use othello_complexity_rs::lib::reverse_common::run_bfs;

fn main() {
    let cfg: Cfg = Cfg::parse();
    if let Err(e) = run_bfs(&cfg) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
