use clap::{Args, Parser, Subcommand};
use othello_complexity_rs::record::GoalMatch;
use othello_complexity_rs::search::run::{run_strategy, ExternalBfs, Strategy};
use std::io;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "reverse_search",
    version,
    about = "Prove position or equivalence-class reachability with reverse search"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Sequential depth-first reverse search.
    Dfs(BasicOpts),
    /// Depth-first search with predecessor ordering.
    #[command(name = "dfs-move-ordering")]
    MoveOrdering(BasicOpts),
    /// Parallel depth-first reverse search.
    #[command(name = "dfs-parallel")]
    DfsPar(ParallelOpts),
    /// Parallel greedy best-first search.
    #[command(name = "gbfs-parallel")]
    GbfsPar(GbfsOpts),
    /// External-memory breadth-first search.
    #[command(name = "bfs-external")]
    ExternalBfs(BfsArgs),
    /// Parallel external-memory breadth-first search.
    #[command(name = "bfs-external-parallel")]
    ExternalBfsPar(BfsArgs),
    /// Parallel in-memory breadth-first search.
    #[command(name = "bfs-inmemory-parallel")]
    InmemoryBfsPar(InmemoryBfsOpts),
}

#[derive(Args, Debug, Clone)]
struct BasicOpts {
    /// File of 64-character relative boards followed by a required b/w turn.
    input: PathBuf,
    #[arg(short, long, default_value = "result")]
    out_dir: PathBuf,
    /// Mode: exact, symmetry (geometry only), or equivalence (geometry and color/turn exchange).
    #[arg(long)]
    mode: GoalMatch,
    /// Forward-search boundary, including passes and terminals at this count.
    #[arg(short = 'd', long, env = "DISCS", default_value_t = 10, value_parser = clap::value_parser!(u32).range(4..=64))]
    discs: u32,
    /// Per-exact-candidate limit: visited nodes in DFS, registered positions in GBFS.
    #[arg(long, env = "MAX_NODES", default_value_t = 1_000_000)]
    max_nodes: usize,
}

#[derive(Args, Debug)]
struct ParallelOpts {
    #[command(flatten)]
    basic: BasicOpts,
    /// Stop memoization after this many visit attempts; search continues.
    #[arg(long, env = "TABLE_SIZE", default_value_t = 100_000)]
    table_size: usize,
    /// Worker threads; zero selects the available parallelism.
    #[arg(long, env = "RAYON_THREADS", default_value_t = 60)]
    threads: usize,
}

#[derive(Args, Debug)]
struct GbfsOpts {
    #[command(flatten)]
    basic: BasicOpts,
    #[arg(long)]
    use_lp: bool,
    /// Worker threads; zero selects available parallelism, capped at 64.
    #[arg(long, env = "RAYON_THREADS", default_value_t = 60)]
    threads: usize,
}

#[derive(Args, Debug)]
struct InmemoryBfsOpts {
    /// File of 64-character relative boards followed by a required b/w turn.
    input: PathBuf,
    #[arg(short, long, default_value = "result")]
    out_dir: PathBuf,
    /// Mode: exact, symmetry (geometry only), or equivalence (geometry and color/turn exchange).
    #[arg(long)]
    mode: GoalMatch,
    /// Forward-search boundary, including passes and terminals at this count.
    #[arg(short = 'd', long, env = "DISCS", default_value_t = 10, value_parser = clap::value_parser!(u32).range(4..=64))]
    discs: u32,
    /// Per-exact-candidate maximum positions in the current and next layers.
    #[arg(long, default_value_t = usize::MAX)]
    max_nodes: usize,
    #[arg(long)]
    use_lp: bool,
    #[arg(long)]
    use_occupancy_cache: bool,
    /// Worker threads; zero selects available parallelism, capped at 64.
    #[arg(long, default_value_t = 0)]
    threads: usize,
}

#[derive(Args, Debug)]
struct BfsArgs {
    /// File of 64-character relative boards followed by a required b/w turn.
    input: PathBuf,
    #[arg(short, long, default_value = "result")]
    out_dir: PathBuf,
    /// Mode: exact, symmetry (geometry only), or equivalence (geometry and color/turn exchange).
    #[arg(long)]
    mode: GoalMatch,
    /// Forward-search boundary, including passes and terminals at this count.
    #[arg(short = 'd', long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(4..=64))]
    discs: u32,
    #[arg(short = 'j', long, default_value_t = 0)]
    jobs: usize,
    #[arg(short, long, default_value_t = 0)]
    verbose: u8,
    /// Maximum input positions expanded together before writing a sorted run.
    #[arg(short = 'b', long, default_value_t = 1_000_000)]
    block_size: usize,
    #[arg(short = 't', long, default_value = "tmp")]
    tmp_dir: PathBuf,
    /// Resume the same input and settings from the committed checkpoint.
    #[arg(short = 'r', long)]
    resume: bool,
}

fn into_basic_strategy(
    opts: BasicOpts,
    make_strategy: impl FnOnce(u32, usize) -> Strategy,
) -> (Strategy, PathBuf, PathBuf, GoalMatch) {
    (
        make_strategy(opts.discs, opts.max_nodes),
        opts.input,
        opts.out_dir,
        opts.mode,
    )
}

fn into_external_strategy(
    opts: BfsArgs,
    make_strategy: impl FnOnce(ExternalBfs) -> Strategy,
) -> (Strategy, PathBuf, PathBuf, GoalMatch) {
    let strategy = make_strategy(ExternalBfs {
        discs: opts.discs,
        jobs: opts.jobs,
        verbose: opts.verbose,
        block_size: opts.block_size,
        tmp_dir: opts.tmp_dir,
        resume: opts.resume,
    });
    (strategy, opts.input, opts.out_dir, opts.mode)
}

fn into_strategy(cli: Cli) -> (Strategy, PathBuf, PathBuf, GoalMatch) {
    match cli.command {
        Command::Dfs(opts) => {
            into_basic_strategy(opts, |discs, max_nodes| Strategy::Dfs { discs, max_nodes })
        }
        Command::MoveOrdering(opts) => into_basic_strategy(opts, |discs, max_nodes| {
            Strategy::DfsMoveOrdering { discs, max_nodes }
        }),
        Command::DfsPar(opts) => {
            into_basic_strategy(opts.basic, |discs, max_nodes| Strategy::DfsParallel {
                discs,
                max_nodes,
                table_size: opts.table_size,
                threads: opts.threads,
            })
        }
        Command::GbfsPar(opts) => {
            into_basic_strategy(opts.basic, |discs, max_nodes| Strategy::GbfsParallel {
                discs,
                max_nodes,
                use_lp: opts.use_lp,
                threads: opts.threads,
            })
        }
        Command::ExternalBfs(opts) => into_external_strategy(opts, Strategy::BfsExternal),
        Command::ExternalBfsPar(opts) => {
            into_external_strategy(opts, Strategy::BfsExternalParallel)
        }
        Command::InmemoryBfsPar(opts) => {
            let strategy = Strategy::BfsInmemoryParallel {
                discs: opts.discs,
                max_nodes: opts.max_nodes,
                use_lp: opts.use_lp,
                use_occupancy_cache: opts.use_occupancy_cache,
                threads: opts.threads,
            };
            (strategy, opts.input, opts.out_dir, opts.mode)
        }
    }
}

fn dispatch(cli: Cli) -> io::Result<()> {
    let (strategy, input, out_dir, mode) = into_strategy(cli);
    run_strategy(&strategy, &input, &out_dir, mode)?;
    Ok(())
}

fn main() {
    if let Err(error) = dispatch(Cli::parse()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
