use clap::Parser;
use othello_complexity_rs::sampling::convert_classes;
use std::io;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Convert an equivalence-class sample to absolute positions")]
struct Cli {
    #[arg(long)]
    sample_dir: PathBuf,
    #[arg(long)]
    out_dir: PathBuf,
    #[arg(long)]
    seed: u64,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    convert_classes(&cli.sample_dir, &cli.out_dir, cli.seed)
}
