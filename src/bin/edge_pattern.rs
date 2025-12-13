use clap::Parser;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use othello_complexity_rs::io::parse_file_to_boards;
use othello_complexity_rs::othello::Board;
use othello_complexity_rs::prunings::impossible_edges::{
    check_edge_patterns, check_edge_patterns_in_board, IMPOSSIBLE_PATTERNS,
};

/// 盤面集合を読み込み、四辺に指定パターンがあるかで仕分ける
#[derive(Parser, Debug)]
#[command(
    name = "edge_pattern",
    about = "Read boards from text and split into edge_OK.txt / edge_NG.txt depending on edge patterns"
)]
struct Cli {
    /// 盤面が並ぶテキストファイルのパス（各行 64 文字の X/O/-）
    #[arg(value_name = "INPUT")]
    input: PathBuf,

    /// 出力先ディレクトリ（デフォルト: crate 直下の result/）
    #[arg(short = 'o', long = "out-dir", value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// 検査パターン（1..8 文字）。複数指定すると OR で判定（例: -p XOXO -p OXOX）
    #[arg(short, long, value_name = "PATTERN")]
    pattern: Vec<String>,
}

fn write_board(writer: &mut BufWriter<File>, b: &Board) -> std::io::Result<()> {
    writeln!(writer, "{}", b.to_string())
}

fn main() -> std::io::Result<()> {
    let args = Cli::parse();
    let boards = parse_file_to_boards(&args.input.to_string_lossy())?;

    let out_dir = args
        .out_dir
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("result"));
    fs::create_dir_all(&out_dir)?;

    let mut edge_ok = BufWriter::new(File::create(out_dir.join("edge_OK.txt"))?);
    let mut edge_ng = BufWriter::new(File::create(out_dir.join("edge_NG.txt"))?);
    let mut board_ok = BufWriter::new(File::create(out_dir.join("edge_in_board_OK.txt"))?);
    let mut board_ng = BufWriter::new(File::create(out_dir.join("edge_in_board_NG.txt"))?);
    let mut summary = BufWriter::new(File::create(out_dir.join("edge_summary.txt"))?);

    let patterns: Vec<&str> = if args.pattern.is_empty() {
        IMPOSSIBLE_PATTERNS.to_vec()
    } else {
        args.pattern.iter().map(|s| s.as_str()).collect()
    };

    let mut ok_cnt = 0usize;
    let mut ng_cnt = 0usize;
    let mut board_ok_cnt = 0usize;
    let mut board_ng_cnt = 0usize;

    for b in &boards {
        let board_str = b.to_string();
        let edge_is_ng = check_edge_patterns(b, &patterns);
        let in_board_is_ng = check_edge_patterns_in_board(b, &patterns);

        if edge_is_ng {
            write_board(&mut edge_ng, b)?;
            ng_cnt += 1;
        } else {
            write_board(&mut edge_ok, b)?;
            ok_cnt += 1;
        }
        if in_board_is_ng {
            write_board(&mut board_ng, b)?;
            board_ng_cnt += 1;
        } else {
            write_board(&mut board_ok, b)?;
            board_ok_cnt += 1;
        }

        writeln!(
            summary,
            "{}, {}, {}",
            board_str,
            if edge_is_ng { "ng" } else { "ok" },
            if in_board_is_ng { "ng" } else { "ok" }
        )?;
    }

    edge_ok.flush()?;
    edge_ng.flush()?;
    board_ok.flush()?;
    board_ng.flush()?;
    summary.flush()?;

    println!(
        "edge done: {} boards (OK: {}, NG: {})",
        boards.len(),
        ok_cnt,
        ng_cnt
    );

    println!(
        "edge_in_board done: {} boards (OK: {}, NG: {})",
        boards.len(),
        board_ok_cnt,
        board_ng_cnt
    );
    Ok(())
}
