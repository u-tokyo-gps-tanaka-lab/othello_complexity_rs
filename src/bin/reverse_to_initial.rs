use std::io;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use othello_complexity_rs::search::bfs_search::Cfg as BfsCfg;
use othello_complexity_rs::search::reverse_common::{
    default_input_path, default_out_dir, read_env_with_default, run_bfs, run_bfs_par, run_dfs,
    run_move_ordering, run_parallel,
};

#[derive(Parser, Debug)]
#[command(
    name = "reverse_to_initial",
    author,
    version,
    about = "Run reverse search strategies for Othello"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    Dfs,
    MoveOrdering,
    Parallel,
    Bfs,
    BfsPar,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Sequential depth-first reverse search (default implementation)
    Dfs(BasicOpts),
    /// Sequential reverse search with move ordering heuristics
    MoveOrdering(BasicOpts),
    /// Parallel reverse search using rayon workers
    Parallel(ParallelOpts),
    /// BFS-based meet-in-the-middle search
    Bfs(BfsArgs),
    /// Parallel BFS search with resume support
    BfsPar(BfsArgs),
}

#[derive(Args, Debug, Clone)]
pub struct BasicOpts {
    /// Input file containing board positions
    #[arg(value_name = "INPUT")]
    input: Option<PathBuf>,

    /// Output directory for result files
    #[arg(short, long, value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// Number of discs at which to stop the forward search
    #[arg(long, value_name = "N")]
    discs: Option<i32>,

    /// Maximum number of nodes to explore in reverse search
    #[arg(long = "max-nodes", value_name = "N")]
    max_nodes: Option<usize>,
}

impl BasicOpts {
    fn resolve(&self) -> (PathBuf, PathBuf, i32, usize) {
        let input = self.input.clone().unwrap_or_else(default_input_path);
        let out_dir = self.out_dir.clone().unwrap_or_else(default_out_dir);
        let discs = self
            .discs
            .unwrap_or_else(|| read_env_with_default("DISCS", 10));
        let max_nodes = self
            .max_nodes
            .unwrap_or_else(|| read_env_with_default("MAX_NODES", 1_000_000usize));
        (input, out_dir, discs, max_nodes)
    }
}

#[derive(Args, Debug, Clone)]
pub struct ParallelOpts {
    #[command(flatten)]
    basic: BasicOpts,

    /// Table size hint for the internal transposition table
    #[arg(long = "table-size", value_name = "N")]
    table_size: Option<usize>,

    /// Number of rayon worker threads (0 = library default)
    #[arg(long, value_name = "N")]
    threads: Option<usize>,
}

impl ParallelOpts {
    fn resolve(&self) -> (PathBuf, PathBuf, i32, usize, usize, Option<usize>) {
        let (input, out_dir, discs, max_nodes) = self.basic.resolve();
        let table_size = self
            .table_size
            .unwrap_or_else(|| read_env_with_default("TABLE_SIZE", 100_000usize));
        let thread_setting = self
            .threads
            .unwrap_or_else(|| read_env_with_default("RAYON_THREADS", 60usize));
        let threads = if thread_setting == 0 {
            None
        } else {
            Some(thread_setting)
        };
        (input, out_dir, discs, max_nodes, table_size, threads)
    }
}

#[derive(Args, Debug, Clone)]
pub struct BfsArgs {
    /// Input file containing board positions
    #[arg(value_name = "INPUT")]
    input: PathBuf,

    /// Output directory for result files
    #[arg(short, long, value_name = "DIR", default_value = "result")]
    out_dir: PathBuf,

    /// Number of worker threads (0 = automatic)
    #[arg(short = 'j', long, default_value_t = 0)]
    jobs: usize,

    /// Verbosity level
    #[arg(short, long, default_value_t = 0)]
    verbose: u8,

    /// Block size for BFS batching
    #[arg(short = 'b', long, default_value_t = 1_000_000)]
    block_size: usize,

    /// Disc threshold for forward search
    #[arg(short = 'd', long, default_value_t = 10)]
    discs: usize,

    /// Temporary directory for intermediate files
    #[arg(short = 't', long, value_name = "DIR", default_value = "tmp")]
    tmp_dir: PathBuf,

    /// Resume from intermediate state
    #[arg(short = 'r', long)]
    resume: bool,
}

impl From<BfsArgs> for BfsCfg {
    fn from(args: BfsArgs) -> Self {
        BfsCfg {
            input: args.input,
            out_dir: args.out_dir,
            jobs: args.jobs,
            verbose: args.verbose,
            block_size: args.block_size,
            discs: args.discs,
            tmp_dir: args.tmp_dir,
            resume: args.resume,
        }
    }
}

fn dispatch(cli: Cli) -> io::Result<()> {
    match cli.command {
        Command::Dfs(opts) => {
            let (input, out_dir, discs, max_nodes) = opts.resolve();
            run_dfs(&input, &out_dir, discs, max_nodes)
        }
        Command::MoveOrdering(opts) => {
            let (input, out_dir, discs, max_nodes) = opts.resolve();
            run_move_ordering(&input, &out_dir, discs, max_nodes)
        }
        Command::Parallel(opts) => {
            let (input, out_dir, discs, max_nodes, table_size, threads) = opts.resolve();
            run_parallel(&input, &out_dir, discs, max_nodes, table_size, threads)
        }
        Command::Bfs(args) => {
            let cfg: BfsCfg = args.into();
            run_bfs(&cfg)
        }
        Command::BfsPar(args) => {
            let cfg: BfsCfg = args.into();
            run_bfs_par(&cfg)
        }
    }
}

fn normalize_strategy_name(name: &str) -> Option<Strategy> {
    let normalized = name.trim().to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "default" | "dfs" => Some(Strategy::Dfs),
        "mo" | "move" | "move-ordering" | "moveordering" => Some(Strategy::MoveOrdering),
        "par" | "parallel" | "rayon" => Some(Strategy::Parallel),
        "bfs" => Some(Strategy::Bfs),
        "bfs-par" | "bfspar" | "bfs-parallel" | "parallel-bfs" => Some(Strategy::BfsPar),
        _ => None,
    }
}

fn strategy_to_subcommand(strategy: Strategy) -> &'static str {
    match strategy {
        Strategy::Dfs => "dfs",
        Strategy::MoveOrdering => "move-ordering",
        Strategy::Parallel => "parallel",
        Strategy::Bfs => "bfs",
        Strategy::BfsPar => "bfs-par",
    }
}

fn recognize_subcommand(arg: &str) -> Option<Strategy> {
    match arg {
        "dfs" => Some(Strategy::Dfs),
        "move-ordering" => Some(Strategy::MoveOrdering),
        "parallel" => Some(Strategy::Parallel),
        "bfs" => Some(Strategy::Bfs),
        "bfs-par" => Some(Strategy::BfsPar),
        _ => None,
    }
}

fn build_arg_vector() -> Vec<String> {
    let mut raw_args: Vec<String> = std::env::args().collect();
    if raw_args.len() <= 1 {
        return raw_args;
    }

    let mut other_args: Vec<String> = Vec::new();
    let mut strategy: Option<Strategy> = None;

    let mut iter = raw_args.clone().into_iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg
            .strip_prefix("--mode=")
            .or_else(|| arg.strip_prefix("--strategy="))
        {
            strategy = normalize_strategy_name(value);
            if strategy.is_none() {
                eprintln!("error: unknown strategy '{}'.", value);
                std::process::exit(2);
            }
            continue;
        }

        if arg == "--mode" || arg == "--strategy" || arg == "-m" {
            let value = iter.next().unwrap_or_else(|| {
                eprintln!("error: missing value for {}.", arg);
                std::process::exit(2);
            });
            strategy = normalize_strategy_name(&value);
            if strategy.is_none() {
                eprintln!("error: unknown strategy '{}'.", value);
                std::process::exit(2);
            }
            continue;
        }

        other_args.push(arg);
    }

    let mut new_args = Vec::with_capacity(raw_args.len());
    new_args.push(raw_args.remove(0));

    if let Some(existing) = other_args.first().and_then(|arg| recognize_subcommand(arg)) {
        if let Some(selected) = strategy {
            if selected != existing {
                eprintln!(
                    "error: conflicting strategy selections: '{}' vs '{}'.",
                    strategy_to_subcommand(selected),
                    strategy_to_subcommand(existing)
                );
                std::process::exit(2);
            }
        }
        strategy = None;
    }

    if let Some(selected) = strategy {
        new_args.push(strategy_to_subcommand(selected).to_string());
    }

    new_args.extend(other_args);
    new_args
}

fn main() {
    let argv = build_arg_vector();
    let cli = Cli::parse_from(argv);
    if let Err(e) = dispatch(cli) {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
