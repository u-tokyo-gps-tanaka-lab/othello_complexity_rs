use clap::Parser;

use othello_complexity_rs::search::bfs::Cfg;
use othello_complexity_rs::search::reverse_common::run_bfs_par;

fn main() {
    let cfg: Cfg = Cfg::parse();
    if let Err(e) = run_bfs_par(&cfg) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
