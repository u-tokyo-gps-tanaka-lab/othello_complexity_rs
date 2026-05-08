use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use clap::Parser;

use othello_complexity_rs::{
    io::parse_file_to_boards,
    symmetry_distinguished::{
        deterministic_sample, distinguished_orbit_key, DistinguishedSample, FULL_ORBIT_SIZE,
    },
};

#[derive(Debug, Parser)]
#[command(
    name = "symmetry_distinguished",
    about = "Generate a symmetry-distinguished sample from symmetry-canonical Othello boards"
)]
struct Args {
    /// Input file containing boards that passed the D4 symmetry representative check
    #[arg(long, value_name = "FILE")]
    sym_ok: PathBuf,

    /// File containing boards classified reachable in the previous overall experiment
    #[arg(long, value_name = "FILE")]
    reachable: Option<PathBuf>,

    /// Output directory for newly generated files
    #[arg(
        short,
        long,
        value_name = "DIR",
        default_value = "result/cog/symmetry_distinguished"
    )]
    out_dir: PathBuf,

    /// Seed for deterministic pseudo-random variant selection
    #[arg(long, default_value_t = 0)]
    seed: u64,

    /// Overwrite files in the output directory
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Default)]
struct Summary {
    input_boards: usize,
    kept_boards: usize,
    dropped_boards: usize,
    full_orbits: usize,
    smaller_orbits: usize,
    reachable_input_boards: usize,
    reachable_matched_samples: usize,
    reachable_dropped_by_orbit_sampling: usize,
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    fs::create_dir_all(&args.out_dir)?;

    let sym_boards = parse_file_to_boards(&args.sym_ok.to_string_lossy())?;
    let reachable_boards = match &args.reachable {
        Some(path) => parse_file_to_boards(&path.to_string_lossy())?,
        None => Vec::new(),
    };
    let reachable_keys = reachable_boards
        .iter()
        .map(|b| distinguished_orbit_key(*b))
        .collect::<HashSet<_>>();

    let mut summary = Summary {
        input_boards: sym_boards.len(),
        reachable_input_boards: reachable_boards.len(),
        ..Summary::default()
    };

    let mut samples_by_orbit = HashMap::<[u64; 2], DistinguishedSample>::new();
    let mut dropped_reachable = HashSet::<[u64; 2]>::new();

    let output_paths = OutputPaths::new(&args.out_dir);
    output_paths.ensure_writable(args.force)?;

    let mut sample_writer = new_output(&output_paths.sample, args.force)?;
    let mut strict_writer = new_output(&output_paths.strict_input, args.force)?;
    let mut dropped_writer = new_output(&output_paths.dropped, args.force)?;
    let mut map_writer = new_output(&output_paths.map, args.force)?;

    writeln!(
        map_writer,
        "original\tselected\torbit_size\tselected_index\tused_for_strict"
    )?;

    for board in sym_boards {
        let orbit_key = distinguished_orbit_key(board);
        match deterministic_sample(board, args.seed) {
            Some(sample) => {
                summary.kept_boards += 1;
                if sample.orbit_size == FULL_ORBIT_SIZE {
                    summary.full_orbits += 1;
                } else {
                    summary.smaller_orbits += 1;
                }

                let used_for_strict = reachable_keys.contains(&orbit_key);
                if used_for_strict {
                    summary.reachable_matched_samples += 1;
                    writeln!(strict_writer, "{}", sample.selected.to_string())?;
                }

                writeln!(sample_writer, "{}", sample.selected.to_string())?;
                writeln!(
                    map_writer,
                    "{}\t{}\t{}\t{}\t{}",
                    sample.original.to_string(),
                    sample.selected.to_string(),
                    sample.orbit_size,
                    sample.selected_index,
                    used_for_strict
                )?;
                samples_by_orbit.insert(orbit_key, sample);
            }
            None => {
                summary.dropped_boards += 1;
                if reachable_keys.contains(&orbit_key) {
                    dropped_reachable.insert(orbit_key);
                }
                writeln!(dropped_writer, "{}", board.to_string())?;
            }
        }
    }

    for reachable in &reachable_boards {
        let key = distinguished_orbit_key(*reachable);
        if !samples_by_orbit.contains_key(&key) && dropped_reachable.contains(&key) {
            summary.reachable_dropped_by_orbit_sampling += 1;
        }
    }

    let mut summary_writer = new_output(&output_paths.summary, args.force)?;
    write_summary(&mut summary_writer, &args, &summary)?;
    write_summary(&mut io::stdout().lock(), &args, &summary)?;

    Ok(())
}

struct OutputPaths {
    sample: PathBuf,
    strict_input: PathBuf,
    dropped: PathBuf,
    map: PathBuf,
    summary: PathBuf,
}

impl OutputPaths {
    fn new(out_dir: &Path) -> Self {
        Self {
            sample: out_dir.join("sample_S.txt"),
            strict_input: out_dir.join("strict_input.txt"),
            dropped: out_dir.join("dropped_self_symmetric.txt"),
            map: out_dir.join("sample_map.tsv"),
            summary: out_dir.join("summary.txt"),
        }
    }

    fn ensure_writable(&self, force: bool) -> io::Result<()> {
        if force {
            return Ok(());
        }

        for path in [
            &self.sample,
            &self.strict_input,
            &self.dropped,
            &self.map,
            &self.summary,
        ] {
            if path.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "refusing to overwrite {}; pass --force to replace it",
                        path.display()
                    ),
                ));
            }
        }
        Ok(())
    }
}

fn new_output(path: &Path, force: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    options.open(path)
}

fn write_summary(mut w: impl Write, args: &Args, summary: &Summary) -> io::Result<()> {
    writeln!(w, "sym_ok = {}", args.sym_ok.display())?;
    if let Some(path) = &args.reachable {
        writeln!(w, "reachable = {}", path.display())?;
    } else {
        writeln!(w, "reachable = <none>")?;
    }
    writeln!(w, "out_dir = {}", args.out_dir.display())?;
    writeln!(w, "seed = {}", args.seed)?;
    writeln!(w, "input_boards = {}", summary.input_boards)?;
    writeln!(w, "kept_boards = {}", summary.kept_boards)?;
    writeln!(w, "dropped_boards = {}", summary.dropped_boards)?;
    writeln!(w, "full_orbits = {}", summary.full_orbits)?;
    writeln!(w, "smaller_orbits = {}", summary.smaller_orbits)?;
    writeln!(
        w,
        "reachable_input_boards = {}",
        summary.reachable_input_boards
    )?;
    writeln!(
        w,
        "reachable_matched_samples = {}",
        summary.reachable_matched_samples
    )?;
    writeln!(
        w,
        "reachable_dropped_by_orbit_sampling = {}",
        summary.reachable_dropped_by_orbit_sampling
    )?;
    Ok(())
}
