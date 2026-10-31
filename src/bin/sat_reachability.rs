use clap::Parser;
use othello_complexity_rs::filters::sat::{run_with_options, RunOptions, SatPortfolio};
use othello_complexity_rs::io::{
    ensure_tri_outputs, format_position, parse_file_to_positions, parse_position,
    reject_estimate_output_dir,
};
use othello_complexity_rs::othello::Position;
use othello_complexity_rs::record::GoalMatch;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "sat_reachability",
    about = "SAT reachability and verified witness search for Othello positions with absolute turns"
)]
struct Cli {
    /// Use the standard initial position with Black to move
    #[arg(long, conflicts_with = "start", required_unless_present = "start")]
    from_initial: bool,

    /// Start position: 64 X/O/- cells followed by a space and b or w (X = side to move)
    #[arg(long, value_name = "POSITION", allow_hyphen_values = true)]
    start: Option<String>,

    /// Goal position in the same format as --start
    #[arg(
        long,
        value_name = "POSITION",
        allow_hyphen_values = true,
        conflicts_with = "goal_file",
        required_unless_present = "goal_file"
    )]
    goal: Option<String>,

    /// Input file containing one goal position per line
    #[arg(long, value_name = "FILE")]
    goal_file: Option<PathBuf>,

    /// Mode: exact, symmetry (geometry only), or equivalence (geometry and color/turn exchange)
    #[arg(long, value_name = "MODE")]
    mode: GoalMatch,

    /// SAT encoding portfolio; join names with '+'. nopass-witness alone is OK/UNKNOWN only
    #[arg(long, value_name = "NAME")]
    encoding: SatPortfolio,

    /// Kissat instances per encoding, using seeds 0 through N - 1
    #[arg(long, default_value_t = 1, value_parser = parse_positive_usize)]
    seeds: usize,

    /// Print coordinates for every layer
    #[arg(long)]
    show_coords: bool,

    /// Print relative X/O/- boards for every layer
    #[arg(long)]
    show_boards: bool,

    /// Print encoding size and timing for each exact candidate
    #[arg(long)]
    verbose: bool,

    /// Maximum number of goal positions solved concurrently
    #[arg(long, default_value_t = 1, value_parser = parse_positive_usize)]
    parallel_goals: usize,

    /// Output directory for classifications and verified records
    #[arg(
        short = 'o',
        long = "out-dir",
        value_name = "DIR",
        default_value = "result"
    )]
    out_dir: PathBuf,

    /// Emit DIMACS CNF files for each exact candidate
    #[arg(long)]
    dump_dimacs_cnf: bool,

    /// Generate DIMACS CNF files without classifications or records
    #[arg(long)]
    cnf_dump_only: bool,

    /// SAT solver timeout in seconds for each exact candidate
    #[arg(long, value_name = "SECS", value_parser = parse_positive_u64)]
    sat_timeout_secs: Option<u64>,
}

fn main() {
    if let Err(err) = run_cli(Cli::parse()) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run_cli(cli: Cli) -> Result<(), String> {
    // Validate every input before opening any output: a bad line must not erase
    // an earlier run's classifications or evidence.
    let start = resolve_start(&cli)?;
    let goals = resolve_goals(&cli)?;
    let goal_match = cli.mode;
    reject_estimate_output_dir(&cli.out_dir)
        .map_err(|e| format!("cannot use output directory {}: {e}", cli.out_dir.display()))?;
    let cnf_dump_dir = (cli.dump_dimacs_cnf || cli.cnf_dump_only).then(|| cli.out_dir.clone());
    print_config(&cli, start, &goals, goal_match, cnf_dump_dir.as_deref());

    let mut outputs = if cli.cnf_dump_only {
        None
    } else {
        let prefix = format!("sat_{}", goal_match.as_str());
        let classifications = ensure_tri_outputs(&cli.out_dir, &prefix).map_err(|e| {
            format!(
                "failed to create output files in {}: {e}",
                cli.out_dir.display()
            )
        })?;
        let path = cli.out_dir.join(format!("{prefix}_records.csv"));
        let records = BufWriter::new(
            File::create(&path).map_err(|e| format!("failed to create {}: {e}", path.display()))?,
        );
        Some((classifications, records))
    };

    run_with_options(
        RunOptions {
            start,
            goals,
            goal_match,
            encodings: cli.encoding,
            seeds: cli.seeds,
            parallel_goals: cli.parallel_goals,
            show_coords: cli.show_coords,
            show_boards: cli.show_boards,
            verbose: cli.verbose,
            cnf_dump_dir,
            cnf_dump_only: cli.cnf_dump_only,
            sat_timeout_per_instance: cli.sat_timeout_secs.map(Duration::from_secs),
        },
        |outcome| {
            let (classifications, records) = outputs
                .as_mut()
                .ok_or("CNF-only execution unexpectedly produced a classification")?;
            if let Some(record) = outcome.record() {
                record.verify().map_err(|e| {
                    format!(
                        "input {}: record verification failed: {e}",
                        outcome.input_index()
                    )
                })?;
                writeln!(records, "{}", record.to_csv())
                    .map_err(|e| format!("failed to write record: {e}"))?;
                records
                    .flush()
                    .map_err(|e| format!("failed to flush records: {e}"))?;
            }
            classifications
                .write_result(outcome, &format_position(outcome.goal()))
                .map_err(|e| format!("failed to write classification: {e}"))?;
            classifications
                .flush()
                .map_err(|e| format!("failed to flush classifications: {e}"))
        },
    )?;
    Ok(())
}

fn print_config(
    cli: &Cli,
    start: Position,
    goals: &[Position],
    goal_match: GoalMatch,
    cnf_dump_dir: Option<&Path>,
) {
    println!("start: {}", format_position(start));
    println!("mode: {goal_match}");
    println!("encoding: {}", cli.encoding);
    println!("seeds per encoding: {}", cli.seeds);
    println!("goals: {}", goals.len());
    println!("parallel goals: {}", cli.parallel_goals);
    println!("output directory: {}", cli.out_dir.display());
    if let Some(path) = &cli.goal_file {
        println!("goal file: {}", path.display());
    }
    if let Some(dir) = cnf_dump_dir {
        println!("CNF directory: {}", dir.display());
    }
    if let Some(secs) = cli.sat_timeout_secs {
        println!("SAT timeout per exact candidate: {secs} s");
    }
}

fn resolve_start(cli: &Cli) -> Result<Position, String> {
    match (cli.from_initial, cli.start.as_deref()) {
        (true, None) => Ok(Position::initial()),
        (false, Some(raw)) => parse_position(raw).map_err(|e| format!("invalid --start: {e}")),
        (true, Some(_)) => Err("--from-initial and --start are mutually exclusive".into()),
        (false, None) => Err("start position is required: use --from-initial or --start".into()),
    }
}

fn resolve_goals(cli: &Cli) -> Result<Vec<Position>, String> {
    match (cli.goal.as_deref(), cli.goal_file.as_deref()) {
        (Some(raw), None) => parse_position(raw)
            .map(|goal| vec![goal])
            .map_err(|e| format!("invalid --goal: {e}")),
        (None, Some(path)) => {
            let path_text = path
                .to_str()
                .ok_or("--goal-file path must be valid UTF-8")?;
            parse_file_to_positions(path_text)
                .map_err(|e| format!("failed to parse goals from {}: {e}", path.display()))
        }
        (Some(_), Some(_)) => Err("--goal and --goal-file are mutually exclusive".into()),
        (None, None) => Err("goal position is required: use --goal or --goal-file".into()),
    }
}

fn parse_positive_usize(raw: &str) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|e| format!("invalid positive integer '{raw}': {e}"))?;
    if value == 0 {
        return Err("value must be >= 1".into());
    }
    Ok(value)
}

fn parse_positive_u64(raw: &str) -> Result<u64, String> {
    let value = raw
        .parse::<u64>()
        .map_err(|e| format!("invalid positive integer '{raw}': {e}"))?;
    if value == 0 {
        return Err("value must be >= 1".into());
    }
    Ok(value)
}
