use std::io;
use std::path::PathBuf;

use clap::Parser;

use othello_complexity_rs::lib::reverse_common::{
    default_input_path, default_out_dir, read_env_with_default, run_parallel1,
};

#[derive(Parser, Debug)]
#[command(
    name = "reverse_to_initial_par",
    about = "Parallel reverse search using rayon"
)]
struct Cli {
    /// Input file containing board positions
    #[arg(value_name = "INPUT")]
    input: Option<PathBuf>,

    /// Output directory for result files
    #[arg(short, long, value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// Number of discs at which to stop the forward search
    #[arg(long, value_name = "N")]
    discs: Option<i32>,

    /// Maximum number of reverse-search nodes
    #[arg(long = "max-nodes", value_name = "N")]
    max_nodes: Option<usize>,

    /// Table size hint for the transposition table
    #[arg(long)]
    use_lp: bool,

    /// Number of rayon worker threads (0 = default)
    #[arg(long, value_name = "N")]
    threads: Option<usize>,
}

fn run(cli: Cli) -> io::Result<()> {
    let input = cli.input.unwrap_or_else(default_input_path);
    let out_dir = cli.out_dir.unwrap_or_else(default_out_dir);
    let discs = cli
        .discs
        .unwrap_or_else(|| read_env_with_default("DISCS", 10));
    let max_nodes = cli
        .max_nodes
        .unwrap_or_else(|| read_env_with_default("MAX_NODES", 1_000_000usize));
    let use_lp = cli.use_lp;
    let thread_setting = cli
        .threads
        .unwrap_or_else(|| read_env_with_default("RAYON_THREADS", 60usize));
    let threads = if thread_setting == 0 {
        None
    } else {
        Some(thread_setting)
    };

    run_parallel1(&input, &out_dir, discs, max_nodes, use_lp, threads)
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
