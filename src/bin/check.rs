use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};

use othello_complexity_rs::io::parse_file_to_boards;
use othello_complexity_rs::othello::Board;
use othello_complexity_rs::prunings::{
    connectivity::is_connected, kissat::is_sat_ok, linear_programming::check_lp,
    occupancy::check_occupancy_with_string, seg3::check_seg3_more,
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
    #[arg(short = 'o', long = "out-dir", value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// Input file(s) containing board positions
    #[arg(value_name = "INPUT")]
    inputs: Vec<PathBuf>,
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
    /// Linear/IP feasibility check
    Lp(LpOpts),
    /// Occupancy-based pruning check
    Occupancy(CommonOpts),
    /// Seg3-more pruning check
    Seg3More(CommonOpts),
    /// SAT pruning check
    Sat(CommonOpts),
    /// Symmetry check
    Sym(CommonOpts),
}

fn resolve_out_dir(dir: &Option<PathBuf>) -> PathBuf {
    dir.clone()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result"))
}

fn process_inputs(
    opts: &CommonOpts,
    mut f: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if opts.inputs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "at least one input file is required",
        ));
    }
    let out_dir = resolve_out_dir(&opts.out_dir);
    for input in &opts.inputs {
        if let Err(e) = f(input, &out_dir) {
            eprintln!("Error processing {}: {}", input.display(), e);
        }
    }
    Ok(())
}

fn to_path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn process_con_file(path: &Path, out_dir: &Path) -> io::Result<()> {
    let boards = parse_file_to_boards(&to_path_string(path))?;
    fs::create_dir_all(out_dir)?;
    let mut okfile = File::create(out_dir.join("con_OK.txt"))?;
    let mut ngfile = File::create(out_dir.join("con_NG.txt"))?;

    for board in boards {
        let line = board.to_string();
        match is_connected(board.player | board.opponent) {
            true => writeln!(okfile, "{}", line)?,
            false => writeln!(ngfile, "{}", line)?,
        }
    }
    Ok(())
}

fn process_lp_file(path: &Path, out_dir: &Path, by_ip_solver: bool) -> io::Result<()> {
    let boards = parse_file_to_boards(&to_path_string(path))?;
    fs::create_dir_all(out_dir)?;
    let prefix = if by_ip_solver { "ip" } else { "lp" };
    let mut okfile = File::create(out_dir.join(format!("{prefix}_OK.txt")))?;
    let mut ngfile = File::create(out_dir.join(format!("{prefix}_NG.txt")))?;

    for board in boards {
        let line = board.to_string();
        if check_lp(board.player, board.opponent, by_ip_solver) {
            writeln!(okfile, "{}", line)?;
        } else {
            writeln!(ngfile, "{}", line)?;
        }
    }
    Ok(())
}

fn process_occupancy_file(path: &Path, out_dir: &Path) -> io::Result<()> {
    let boards = parse_file_to_boards(&to_path_string(path))?;
    fs::create_dir_all(out_dir)?;
    let mut okfile = File::create(out_dir.join("occupancy_OK.txt"))?;
    let mut ngfile = File::create(out_dir.join("occupancy_NG.txt"))?;
    let mut okfile_ex = File::create(out_dir.join("occupancy_OK_explainable.txt"))?;
    let mut ngfile_ex = File::create(out_dir.join("occupancy_NG_explainable.txt"))?;

    for board in boards {
        let (ok, text) = check_occupancy_with_string(board.player | board.opponent);
        let line = board.to_string();
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

fn process_seg3more_file(path: &Path, out_dir: &Path) -> io::Result<()> {
    let boards = parse_file_to_boards(&to_path_string(path))?;
    fs::create_dir_all(out_dir)?;
    let mut okfile = File::create(out_dir.join("seg3more_OK.txt"))?;
    let mut ngfile = File::create(out_dir.join("seg3more_NG.txt"))?;

    for board in boards {
        let line = board.to_string();
        if check_seg3_more(board.player, board.opponent) {
            writeln!(okfile, "{}", line)?;
        } else {
            writeln!(ngfile, "{}", line)?;
        }
    }
    Ok(())
}

fn process_sat_file(path: &Path, out_dir: &Path) -> io::Result<()> {
    let boards = parse_file_to_boards(&to_path_string(path))?;
    fs::create_dir_all(out_dir)?;
    let mut okfile = File::create(out_dir.join("sat_OK.txt"))?;
    let mut ngfile = File::create(out_dir.join("sat_NG.txt"))?;

    for (index, board) in boards.iter().enumerate() {
        let line = board.to_string();
        match is_sat_ok(index, &line) {
            Ok(true) => {
                println!("SAT: {}", line);
                writeln!(okfile, "{}", line)?;
            }
            Ok(false) => {
                println!("UNSAT: {}", line);
                writeln!(ngfile, "{}", line)?;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    }
    Ok(())
}

fn process_sym_file(path: &Path, out_dir: &Path) -> io::Result<()> {
    let boards = parse_file_to_boards(&to_path_string(path))?;
    fs::create_dir_all(out_dir)?;
    let mut okfile = File::create(out_dir.join("sym_OK.txt"))?;
    let mut ngfile = File::create(out_dir.join("sym_NG.txt"))?;

    for board in boards {
        let line = board.to_string();
        if is_sym_ok(&board)? {
            writeln!(okfile, "{}", line)?;
        } else {
            writeln!(ngfile, "{}", line)?;
        }
    }
    Ok(())
}

fn is_sym_ok(board: &Board) -> io::Result<bool> {
    let mut tmp = [0u64, 0u64];
    let occupied = board.player | board.opponent;
    for i in 1..8 {
        board.board_symmetry(i, &mut tmp);
        let o1 = tmp[0] | tmp[1];
        if o1 < occupied {
            return Ok(false);
        } else if o1 > occupied {
            continue;
        }
        if tmp[0] < board.player {
            return Ok(false);
        } else if tmp[0] > board.player {
            continue;
        }
    }
    Ok(true)
}

fn main() {
    for (i, arg) in std::env::args().enumerate() {
        println!("argv[{}] : {}", i, arg);
    }

    let cli = Cli::parse();
    let result = match cli.command {
        Command::Con(opts) => process_inputs(&opts, process_con_file),
        Command::Lp(opts) => process_inputs(&opts.common, |path, out_dir| {
            process_lp_file(path, out_dir, opts.ip)
        }),
        Command::Occupancy(opts) => process_inputs(&opts, process_occupancy_file),
        Command::Seg3More(opts) => process_inputs(&opts, process_seg3more_file),
        Command::Sat(opts) => process_inputs(&opts, process_sat_file),
        Command::Sym(opts) => process_inputs(&opts, process_sym_file),
    };

    if let Err(e) = result {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
