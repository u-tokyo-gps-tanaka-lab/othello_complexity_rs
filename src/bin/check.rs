use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use rayon::prelude::*;

use othello_complexity_rs::io::{format_position, parse_file_to_positions, parse_position};
use othello_complexity_rs::othello::Board;
use othello_complexity_rs::prunings::{
    connectivity::is_connected, flip_sat::is_sat_ok, impossible_edges::check_edge_patterns,
    linear_programming::check_lp, occupancy::check_occupancy_with_string, seg3::check_seg3_more,
};

#[derive(Parser, Debug)]
#[command(
    name = "check",
    about = "Run various Othello board validation checks over input files"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Debug, Clone)]
struct CommonOpts {
    /// Output directory for result files
    #[arg(
        short = 'o',
        long = "out-dir",
        value_name = "DIR",
        conflicts_with = "board"
    )]
    out_dir: Option<PathBuf>,

    /// Input file(s) containing board positions
    #[arg(value_name = "INPUT", conflicts_with = "board")]
    inputs: Vec<PathBuf>,

    /// Single position: 64 X/O/- cells, one space, and b or w (X = side to move)
    #[arg(long = "board", value_name = "BOARD", allow_hyphen_values = true)]
    board: Option<String>,
}

#[derive(Args, Debug, Clone)]
struct LpOpts {
    #[command(flatten)]
    common: CommonOpts,

    /// Use integer programming solver instead of linear programming
    #[arg(long = "ip")]
    ip: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Connectivity check
    Con(CommonOpts),
    /// Impossible edge-pattern pruning check (physical edges only)
    Edge(CommonOpts),
    /// Linear/IP feasibility check
    Lp(LpOpts),
    /// Occupancy-based pruning check
    Occupancy(CommonOpts),
    /// Seg3more pruning check
    #[command(name = "seg3more")]
    Seg3More(CommonOpts),
    /// flip-based SAT pruning check
    #[command(name = "flip-sat")]
    FlipSat(CommonOpts),
}

fn resolve_out_dir(dir: &Option<PathBuf>) -> PathBuf {
    dir.clone()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result"))
}

fn parse_board_arg(raw: &str) -> io::Result<Board> {
    parse_position(raw)
        .map(|position| position.board)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn print_direct_status(ok: bool) -> io::Result<()> {
    let status = if ok { "OK" } else { "NG" };
    writeln!(io::stdout().lock(), "{status}")
}

fn process_inputs_or_board(
    opts: &CommonOpts,
    mut file_fn: impl FnMut(&Path, &Path) -> io::Result<()>,
    mut board_fn: impl FnMut(&Board) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(board_text) = &opts.board {
        let board = parse_board_arg(board_text)?;
        return board_fn(&board);
    }
    if opts.inputs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "at least one input file or --board is required",
        ));
    }
    let out_dir = resolve_out_dir(&opts.out_dir);
    for input in &opts.inputs {
        if let Err(e) = file_fn(input, &out_dir) {
            eprintln!("Error processing {}: {}", input.display(), e);
        }
    }
    Ok(())
}

fn to_path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn process_file_simple(
    path: &Path,
    out_dir: &Path,
    prefix: &str,
    parallel: bool,
    check: impl Fn(&Board) -> io::Result<bool> + Sync + Send,
) -> io::Result<()> {
    let boards = parse_file_to_positions(&to_path_string(path))?;
    fs::create_dir_all(out_dir)?;
    let mut okfile = File::create(out_dir.join(format!("{prefix}_OK.txt")))?;
    let mut ngfile = File::create(out_dir.join(format!("{prefix}_NG.txt")))?;

    let judged: Vec<io::Result<(String, bool)>> = if parallel {
        boards
            .par_iter()
            .map(|b| check(&b.board).map(|ok| (format_position(*b), ok)))
            .collect()
    } else {
        boards
            .iter()
            .map(|b| check(&b.board).map(|ok| (format_position(*b), ok)))
            .collect()
    };

    for result in judged {
        let (line, ok) = result?;
        if ok {
            writeln!(okfile, "{}", line)?;
        } else {
            writeln!(ngfile, "{}", line)?;
        }
    }
    Ok(())
}

fn run_check(
    opts: &CommonOpts,
    prefix: &str,
    parallel: bool,
    check: impl Fn(&Board) -> io::Result<bool> + Sync + Send,
) -> io::Result<()> {
    if let Some(board_text) = &opts.board {
        let board = parse_board_arg(board_text)?;
        return print_direct_status(check(&board)?);
    }
    if opts.inputs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "at least one input file or --board is required",
        ));
    }
    let out_dir = resolve_out_dir(&opts.out_dir);
    for input in &opts.inputs {
        if let Err(e) = process_file_simple(input, &out_dir, prefix, parallel, &check) {
            eprintln!("Error processing {}: {}", input.display(), e);
        }
    }
    Ok(())
}

fn process_occupancy_file(path: &Path, out_dir: &Path) -> io::Result<()> {
    let boards = parse_file_to_positions(&to_path_string(path))?;
    fs::create_dir_all(out_dir)?;
    let mut okfile = File::create(out_dir.join("occupancy_OK.txt"))?;
    let mut ngfile = File::create(out_dir.join("occupancy_NG.txt"))?;
    let mut okfile_ex = File::create(out_dir.join("occupancy_OK_explainable.txt"))?;
    let mut ngfile_ex = File::create(out_dir.join("occupancy_NG_explainable.txt"))?;

    for position in boards {
        let board = position.board;
        let (ok, text) = check_occupancy_with_string(board.player | board.opponent);
        let line = format_position(position);
        if ok {
            writeln!(okfile, "{}", line)?;
            writeln!(okfile_ex, "{}", text)?;
        } else {
            writeln!(ngfile, "{}", line)?;
            writeln!(ngfile_ex, "{}", text)?;
        }
    }
    Ok(())
}

fn process_occupancy_board(board: &Board) -> io::Result<()> {
    let (ok, text) = check_occupancy_with_string(board.player | board.opponent);
    writeln!(
        io::stdout().lock(),
        "{}\n{}",
        if ok { "OK" } else { "NG" },
        text
    )
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Con(opts) => run_check(&opts, "con", true, |b| {
            Ok(is_connected(b.player | b.opponent))
        }),
        Command::Edge(opts) => run_check(&opts, "edge", true, |b| {
            Ok(check_edge_patterns(b.player, b.opponent))
        }),
        Command::Lp(opts) => {
            run_check(&opts.common, if opts.ip { "ip" } else { "lp" }, true, |b| {
                Ok(check_lp(b.player, b.opponent, opts.ip, false))
            })
        }
        Command::Occupancy(opts) => {
            process_inputs_or_board(&opts, process_occupancy_file, process_occupancy_board)
        }
        Command::Seg3More(opts) => run_check(&opts, "seg3more", false, |b| {
            Ok(check_seg3_more(b.player, b.opponent, false))
        }),
        Command::FlipSat(opts) => run_check(&opts, "flip_sat", false, |b| {
            is_sat_ok(b.player, b.opponent, true)
        }),
    };

    if let Err(e) = result {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
