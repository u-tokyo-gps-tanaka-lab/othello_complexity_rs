use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use rayon::prelude::*;

use othello_complexity_rs::filters::{
    flip_sat::check_flip_sat,
    linear_programming::check_lp,
    occupancy::{occupied_to_string, reachable_occupancy},
    Filter, FilterResult,
};
use othello_complexity_rs::io::{
    format_position, parse_file_to_positions, parse_position, reject_estimate_output_dir,
};
use othello_complexity_rs::othello::{Board, Position};

#[derive(Parser, Debug)]
#[command(
    name = "run_filters",
    about = "Check necessary conditions; OK means only that the position passed this filter"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Debug, Clone)]
struct CommonOpts {
    /// Directory for new classification files and the per-input result record
    #[arg(
        short = 'o',
        long = "out-dir",
        value_name = "DIR",
        conflicts_with = "position",
        required_unless_present = "position"
    )]
    out_dir: Option<PathBuf>,

    /// One input file, containing one absolute position per line
    #[arg(
        value_name = "INPUT",
        conflicts_with = "position",
        required_unless_present = "position"
    )]
    input: Option<PathBuf>,

    /// One position: 64 X/O/- cells, a space, and b or w (X = side to move)
    #[arg(long, value_name = "POSITION", allow_hyphen_values = true)]
    position: Option<String>,
}

#[derive(Args, Debug)]
struct LpOpts {
    #[command(flatten)]
    common: CommonOpts,

    /// Use integer programming instead of the continuous relaxation
    #[arg(long)]
    ip: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Connectivity of occupied squares
    Con(CommonOpts),
    /// Impossible patterns on physical and virtual edges
    Edge(CommonOpts),
    /// Linear/integer programming feasibility
    Lp(LpOpts),
    /// Occupancy reachability; diagnostic masks are separate from positions
    Occupancy(CommonOpts),
    /// Flip consistency, including its occupancy prerequisite
    #[command(name = "seg3more")]
    Seg3More(CommonOpts),
    /// Flip-based SAT relaxation
    #[command(name = "flip-sat")]
    FlipSat(CommonOpts),
}

fn create_output(path: &Path) -> io::Result<BufWriter<File>> {
    Ok(BufWriter::new(File::create(path)?))
}

fn write_results(
    out_dir: &Path,
    prefix: &str,
    positions: &[Position],
    results: &[FilterResult],
) -> io::Result<()> {
    let mut names = vec![
        format!("{prefix}_OK.txt"),
        format!("{prefix}_NG.txt"),
        format!("{prefix}_results.csv"),
    ];
    if prefix == "occupancy" {
        names.extend([
            "occupancy_OK_explainable.csv".to_owned(),
            "occupancy_NG_explainable.csv".to_owned(),
        ]);
    }
    fs::create_dir_all(out_dir)?;
    let mut ok = create_output(&out_dir.join(&names[0]))?;
    let mut ng = create_output(&out_dir.join(&names[1]))?;
    let mut record = csv::Writer::from_writer(create_output(&out_dir.join(&names[2]))?);
    record.write_record(["input_index", "position", "status", "reason"])?;
    let mut masks = if prefix == "occupancy" {
        let mut ok_masks = csv::Writer::from_writer(create_output(&out_dir.join(&names[3]))?);
        let mut ng_masks = csv::Writer::from_writer(create_output(&out_dir.join(&names[4]))?);
        for writer in [&mut ok_masks, &mut ng_masks] {
            writer.write_record([
                "input_index",
                "position",
                "occupied_mask_G_dash",
                "explained_mask_G_dash",
            ])?;
        }
        Some((ok_masks, ng_masks))
    } else {
        None
    };
    for (index, (position, result)) in positions.iter().zip(results).enumerate() {
        let line = format_position(*position);
        writeln!(if result.passes() { &mut ok } else { &mut ng }, "{line}")?;
        record.write_record([
            &(index + 1).to_string(),
            &line,
            result.status(),
            result.reason(),
        ])?;
        if let Some((ok_masks, ng_masks)) = &mut masks {
            let occupied = position.board.player | position.board.opponent;
            let writer = if result.passes() { ok_masks } else { ng_masks };
            writer.write_record([
                (index + 1).to_string(),
                line,
                occupied_to_string(occupied),
                occupied_to_string(reachable_occupancy(occupied)),
            ])?;
        }
    }
    ok.flush()?;
    ng.flush()?;
    record.flush()?;
    if let Some((mut ok_masks, mut ng_masks)) = masks {
        ok_masks.flush()?;
        ng_masks.flush()?;
    }
    Ok(())
}

fn run_check(
    opts: &CommonOpts,
    prefix: &str,
    check: impl Fn(&Board) -> io::Result<FilterResult> + Sync,
) -> io::Result<()> {
    if let Some(text) = &opts.position {
        let position = parse_position(text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let result = check(&position.board)?;
        writeln!(
            io::stdout().lock(),
            "{}",
            if result.passes() { "OK" } else { "NG" }
        )?;
        if !result.reason().is_empty() {
            writeln!(
                io::stderr().lock(),
                "{}: {}",
                result.status(),
                result.reason()
            )?;
        }
        return Ok(());
    }
    let input = opts
        .input
        .as_ref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "input is required"))?;
    let out_dir = opts
        .out_dir
        .as_ref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--out-dir is required"))?;
    // Validate and solve the whole input before opening outputs. A solver failure
    // must never become an NG row, or leave a successful partial classification.
    let positions = parse_file_to_positions(&input.to_string_lossy())?;
    reject_estimate_output_dir(out_dir)?;
    let results: io::Result<Vec<_>> = positions
        .par_iter()
        .enumerate()
        .map(|(index, position)| {
            check(&position.board).map_err(|error| {
                io::Error::new(error.kind(), format!("line {}: {error}", index + 1))
            })
        })
        .collect();
    write_results(out_dir, prefix, &positions, &results?)
}

fn run_filter(opts: &CommonOpts, filter: Filter) -> io::Result<()> {
    run_check(opts, filter.name(), |board| filter.check(board))
}

fn main() {
    let result = match Cli::parse().command {
        Command::Con(opts) => run_filter(&opts, Filter::Con),
        Command::Edge(opts) => run_filter(&opts, Filter::Edge),
        Command::Lp(opts) if opts.ip => run_check(&opts.common, "ip", |board| {
            check_lp(board.player, board.opponent, true, false)
        }),
        Command::Lp(opts) => run_filter(&opts.common, Filter::Lp),
        Command::Occupancy(opts) => run_filter(&opts, Filter::Occupancy),
        Command::Seg3More(opts) => run_filter(&opts, Filter::Seg3More),
        Command::FlipSat(opts) => run_check(&opts, "flip_sat", |board| {
            check_flip_sat(board.player, board.opponent)
        }),
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undetermined_filter_result_is_written_to_ok_with_a_recorded_reason() {
        let directory =
            std::env::temp_dir().join(format!("run-filters-undetermined-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let input = directory.join("input.txt");
        let line = format!("{}\n", format_position(Position::initial()));
        fs::write(&input, &line).unwrap();
        let out_dir = directory.join("output");
        let options = CommonOpts {
            input: Some(input),
            out_dir: Some(out_dir.clone()),
            position: None,
        };
        run_check(&options, "lp", |_| {
            Ok(FilterResult::Undetermined {
                reason: "solver reported \"time limit\", retry\nlater".to_owned(),
            })
        })
        .unwrap();
        assert_eq!(fs::read_to_string(out_dir.join("lp_OK.txt")).unwrap(), line);
        assert!(fs::read(out_dir.join("lp_NG.txt")).unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(out_dir.join("lp_results.csv")).unwrap(),
            format!(
                "input_index,position,status,reason\n1,{},undetermined,\"solver reported \"\"time limit\"\", retry\nlater\"\n",
                format_position(Position::initial()),
            ),
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn solver_errors_leave_no_classification_files() {
        let directory =
            std::env::temp_dir().join(format!("run-filters-solver-error-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let input = directory.join("input.txt");
        fs::write(
            &input,
            format!("{}\n", format_position(Position::initial())),
        )
        .unwrap();
        let out_dir = directory.join("output");
        let options = CommonOpts {
            input: Some(input),
            out_dir: Some(out_dir.clone()),
            position: None,
        };
        let result = run_check(&options, "lp", |_| Err(io::Error::other("solver failed")));
        assert!(result.is_err());
        assert!(!out_dir.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
