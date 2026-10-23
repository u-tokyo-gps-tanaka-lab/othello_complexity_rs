use clap::Parser;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use othello_complexity_rs::io::{format_position, parse_file_to_positions};
use othello_complexity_rs::filters::impossible_edges::check_edge_patterns;

/// Split positions by the necessary condition on physical edge patterns.
#[derive(Parser, Debug)]
#[command(
    name = "edge_pattern",
    about = "Classify absolute positions; OK is filter passage only"
)]
struct Cli {
    /// One input file, with 64 X/O/- cells followed by a space and b or w per line
    #[arg(value_name = "INPUT")]
    input: PathBuf,

    /// Directory for new edge_OK.txt and edge_NG.txt files
    #[arg(short = 'o', long = "out-dir", value_name = "DIR")]
    out_dir: PathBuf,
}

fn main() -> io::Result<()> {
    let args = Cli::parse();
    let positions = parse_file_to_positions(&args.input.to_string_lossy())?;
    let ok_path = args.out_dir.join("edge_OK.txt");
    let ng_path = args.out_dir.join("edge_NG.txt");
    for path in [&ok_path, &ng_path] {
        if path.try_exists()? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("output already exists: {}", path.display()),
            ));
        }
    }
    fs::create_dir_all(&args.out_dir)?;
    let mut ok = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(ok_path)?,
    );
    let mut ng = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(ng_path)?,
    );
    for position in positions {
        let passes = check_edge_patterns(position.board.player, position.board.opponent);
        writeln!(
            if passes { &mut ok } else { &mut ng },
            "{}",
            format_position(position)
        )?;
    }
    ok.flush()?;
    ng.flush()
}
