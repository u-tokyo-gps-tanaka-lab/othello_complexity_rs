use clap::Parser;
use othello_complexity_rs::io::format_position;
use othello_complexity_rs::sampling::position_from_rank;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;

/// 各行の絶対局面rankを読み、手番付き局面を出力する。
#[derive(Debug, Parser)]
#[command(
    name = "num2str",
    version,
    about = "Decode absolute-position ranks; the low bit is the absolute turn"
)]
struct Args {
    /// 入力ファイルのパス（"-" なら標準入力）
    #[arg(value_name = "INPUT")]
    input: PathBuf,
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    // 入力を開く
    let reader: Box<dyn BufRead> = if args.input.as_os_str() == "-" {
        Box::new(BufReader::new(io::stdin()))
    } else {
        let f = File::open(&args.input)?;
        Box::new(BufReader::new(f))
    };

    // 出力（標準出力）をバッファリング
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());

    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        let rank: u128 = text.parse().unwrap();
        let position = position_from_rank(rank).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line {}: {error}", index + 1),
            )
        })?;
        writeln!(out, "{}", format_position(position))?;
    }

    out.flush()?;
    Ok(())
}
