use clap::Parser;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use othello_complexity_rs::lib::othello::Board;

/// 入力ファイルの各行を u128 として読み、val2str() の結果を出力します。
#[derive(Debug, Parser)]
#[command(name = "num2str", version, about = "Read u128 per line and print val2str()")]
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

    // 各行処理
    for (lineno, line_res) in reader.lines().enumerate() {
        let line_no = lineno + 1;
        let line = line_res?;

        // 空行はスキップ（必要なら削除）
        let s = line.trim();
        if s.is_empty() {
            continue;
        }

        // 10進として u128 にパース
        let val: u128 = s.parse().unwrap();

        // ユーザ実装の処理関数
        let out_str = val2str(val);

        // 出力
        writeln!(out, "{}", out_str)?;
    }

    out.flush()?;
    Ok(())
}

/// ここに目的の変換ロジックを書く
fn val2str(mut rank: u128) -> String {
    let (mut player, mut opponent) = (0u64, 0u64);
    for y in 0..8 {
        for x in 0..8 {
            let mask = 1u64 << (y * 8 + x);
            let d: u128 = if 3 <= x && x <= 4 && 3 <= y && y <= 4 {2} else {3};
            let v: u128 = rank % d; if d == 3 {
                if v == 1 {
                    player |= mask;
                } else if v == 2 {
                    opponent |= mask;
                }
            } else {
                if v == 0 {
                    player |= mask;
                } else {
                    opponent |= mask;
                }
            }
            rank /= d;
        }
    }
    let b = Board::new(player, opponent);
    format!("{}", b.to_string())
}