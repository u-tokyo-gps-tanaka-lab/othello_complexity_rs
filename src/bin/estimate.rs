use std::num::{NonZeroU64, NonZeroUsize};
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use othello_complexity_rs::estimate;
use othello_complexity_rs::filters::sat::SatPortfolio;
use othello_complexity_rs::search::run::{ExternalBfs, Strategy};

const DEFAULT_ALPHA: f64 = 0.005;

#[derive(Debug, Parser)]
#[command(
    name = "estimate",
    about = "Estimate the number of reachable Othello positions"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Apply the fixed filter pipeline to a sample.
    Filters(FiltersArgs),
    /// Apply complete SAT classification or an OK-only witness search to unresolved positions.
    Sat(SatArgs),
    /// Classify the preceding stage's unknown positions with reverse search.
    #[command(name = "reverse-search")]
    ReverseSearch(ReverseSearchArgs),
}

#[derive(Debug, Args)]
struct FiltersArgs {
    #[arg(long, value_name = "DIR")]
    sample: PathBuf,

    #[arg(long, value_name = "DIR")]
    out_dir: PathBuf,

    /// Significance level (two-sided alpha); 0.005 gives a 99.5% CI
    #[arg(long, value_name = "A", default_value_t = DEFAULT_ALPHA)]
    alpha: f64,
}

#[derive(Debug, Args)]
struct SatArgs {
    #[arg(long, value_name = "DIR")]
    from: PathBuf,

    #[arg(long, value_name = "DIR")]
    out_dir: PathBuf,

    /// Significance level (two-sided alpha); 0.005 gives a 99.5% CI
    #[arg(long, value_name = "A", default_value_t = DEFAULT_ALPHA)]
    alpha: f64,

    /// SAT encoding portfolio; join names with '+'. nopass-witness alone is OK/UNKNOWN only.
    #[arg(long, value_name = "NAME")]
    encoding: SatPortfolio,

    /// Kissat instances per encoding, using seeds 0 through N - 1.
    #[arg(long, value_name = "N", default_value = "1")]
    seeds: NonZeroUsize,

    /// Maximum number of goal positions solved concurrently.
    #[arg(long, value_name = "N")]
    parallel_goals: NonZeroUsize,

    /// SAT timeout in seconds for each exact candidate.
    #[arg(long, value_name = "S")]
    sat_timeout_secs: Option<NonZeroU64>,
}

#[derive(Debug, Args)]
struct ReverseSearchArgs {
    #[arg(long, value_name = "DIR")]
    from: PathBuf,

    #[arg(long, value_name = "DIR")]
    out_dir: PathBuf,

    /// Significance level (two-sided alpha); 0.005 gives a 99.5% CI
    #[arg(long, value_name = "A", default_value_t = DEFAULT_ALPHA)]
    alpha: f64,

    #[command(subcommand)]
    strategy: StrategyArgs,
}

#[derive(Debug, Subcommand)]
enum StrategyArgs {
    /// Sequential depth-first reverse search.
    Dfs(BasicOpts),
    /// Depth-first search with predecessor ordering.
    #[command(name = "dfs-move-ordering")]
    DfsMoveOrdering(BasicOpts),
    /// Parallel depth-first reverse search.
    #[command(name = "dfs-parallel")]
    DfsParallel(ParallelOpts),
    /// Parallel greedy best-first search.
    #[command(name = "gbfs-parallel")]
    GbfsParallel(GbfsOpts),
    /// External-memory breadth-first search.
    #[command(name = "bfs-external")]
    BfsExternal(ExternalBfsOpts),
    /// Parallel external-memory breadth-first search.
    #[command(name = "bfs-external-parallel")]
    BfsExternalParallel(ExternalBfsOpts),
    /// Parallel in-memory breadth-first search.
    #[command(name = "bfs-inmemory-parallel")]
    BfsInmemoryParallel(InmemoryBfsOpts),
}

#[derive(Debug, Args)]
struct BasicOpts {
    /// Forward-search boundary, including passes and terminals at this count.
    #[arg(
        short = 'd',
        long,
        value_parser = clap::value_parser!(u32).range(4..=64)
    )]
    discs: u32,

    /// Per-exact-candidate limit: visited nodes in DFS, registered positions in GBFS.
    #[arg(long)]
    max_nodes: usize,
}

#[derive(Debug, Args)]
struct ParallelOpts {
    #[command(flatten)]
    basic: BasicOpts,

    /// Stop memoization after this many visit attempts; search continues.
    #[arg(long)]
    table_size: usize,

    /// Worker threads; zero selects the available parallelism.
    #[arg(long)]
    threads: usize,
}

#[derive(Debug, Args)]
struct GbfsOpts {
    #[command(flatten)]
    basic: BasicOpts,

    #[arg(long)]
    use_lp: bool,

    /// Worker threads; zero selects available parallelism, capped at 64.
    #[arg(long)]
    threads: usize,
}

#[derive(Debug, Args)]
struct ExternalBfsOpts {
    /// Forward-search boundary, including passes and terminals at this count.
    #[arg(
        short = 'd',
        long,
        value_parser = clap::value_parser!(u32).range(4..=64)
    )]
    discs: u32,

    #[arg(short = 'j', long)]
    jobs: usize,

    #[arg(short = 'v', long)]
    verbose: u8,

    /// Maximum input positions expanded together before writing a sorted run.
    #[arg(short = 'b', long)]
    block_size: usize,

    #[arg(short = 't', long)]
    tmp_dir: PathBuf,

    /// Resume the same input and settings from the committed checkpoint.
    #[arg(short = 'r', long)]
    resume: bool,
}

#[derive(Debug, Args)]
struct InmemoryBfsOpts {
    /// Forward-search boundary, including passes and terminals at this count.
    #[arg(
        short = 'd',
        long,
        value_parser = clap::value_parser!(u32).range(4..=64)
    )]
    discs: u32,

    /// Per-exact-candidate maximum positions in the current and next layers.
    #[arg(long)]
    max_nodes: usize,

    #[arg(long)]
    use_lp: bool,

    #[arg(long)]
    use_occupancy_cache: bool,

    /// Worker threads; zero selects available parallelism, capped at 64.
    #[arg(long)]
    threads: usize,
}

impl StrategyArgs {
    fn into_strategy(self) -> Strategy {
        match self {
            Self::Dfs(opts) => Strategy::Dfs {
                discs: opts.discs,
                max_nodes: opts.max_nodes,
            },
            Self::DfsMoveOrdering(opts) => Strategy::DfsMoveOrdering {
                discs: opts.discs,
                max_nodes: opts.max_nodes,
            },
            Self::DfsParallel(opts) => Strategy::DfsParallel {
                discs: opts.basic.discs,
                max_nodes: opts.basic.max_nodes,
                table_size: opts.table_size,
                threads: opts.threads,
            },
            Self::GbfsParallel(opts) => Strategy::GbfsParallel {
                discs: opts.basic.discs,
                max_nodes: opts.basic.max_nodes,
                use_lp: opts.use_lp,
                threads: opts.threads,
            },
            Self::BfsExternal(opts) => Strategy::BfsExternal(opts.into_external_bfs()),
            Self::BfsExternalParallel(opts) => {
                Strategy::BfsExternalParallel(opts.into_external_bfs())
            }
            Self::BfsInmemoryParallel(opts) => Strategy::BfsInmemoryParallel {
                discs: opts.discs,
                max_nodes: opts.max_nodes,
                use_lp: opts.use_lp,
                use_occupancy_cache: opts.use_occupancy_cache,
                threads: opts.threads,
            },
        }
    }
}

impl ExternalBfsOpts {
    fn into_external_bfs(self) -> ExternalBfs {
        ExternalBfs {
            discs: self.discs,
            jobs: self.jobs,
            verbose: self.verbose,
            block_size: self.block_size,
            tmp_dir: self.tmp_dir,
            resume: self.resume,
        }
    }
}

fn execute(cli: Cli) -> std::io::Result<()> {
    let interval = match cli.command {
        Command::Filters(args) => estimate::filters(&args.sample, &args.out_dir, args.alpha)?,
        Command::Sat(args) => estimate::sat(
            &args.from,
            &args.out_dir,
            args.alpha,
            args.encoding,
            args.seeds.get(),
            args.parallel_goals.get(),
            args.sat_timeout_secs
                .map(|seconds| Duration::from_secs(seconds.get())),
        )?,
        Command::ReverseSearch(args) => {
            let strategy = args.strategy.into_strategy();
            estimate::reverse_search(&args.from, &args.out_dir, args.alpha, &strategy)?
        }
    };
    println!("{interval}");
    Ok(())
}

fn main() {
    if let Err(error) = execute(Cli::parse()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
