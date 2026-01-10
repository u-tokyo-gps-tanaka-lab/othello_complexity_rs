use clap::Parser;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use othello_complexity_rs::io::parse_file_to_boards;
use othello_complexity_rs::othello::Board;
use othello_complexity_rs::prunings::impossible_edges::{
    check_edge_patterns,
    // check_virtual_edge_patterns,
};

/// 盤面集合を読み込み、四辺に IMPOSSIBLE_PATTERNS があるかで仕分ける
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
    // let mut virtual_edge_ok = BufWriter::new(File::create(out_dir.join("virtual_edge_OK.txt"))?);
    // let mut virtual_edge_ng = BufWriter::new(File::create(out_dir.join("virtual_edge_NG.txt"))?);
    // let mut summary = BufWriter::new(File::create(out_dir.join("edge_summary.txt"))?);

    let mut ok_cnt = 0usize;
    let mut ng_cnt = 0usize;
    // let mut virtual_ok_cnt = 0usize;
    // let mut virtual_ng_cnt = 0usize;

    for b in &boards {
        let edge_is_ok = check_edge_patterns(b.player, b.opponent);
        // let virtual_edge_is_ok = check_virtual_edge_patterns(b.player, b.opponent);

        if edge_is_ok {
            write_board(&mut edge_ok, b)?;
            ok_cnt += 1;
        } else {
            write_board(&mut edge_ng, b)?;
            println!("{}", b.show());
            println!("{}", b.to_string());
            println!("==========\n");
            ng_cnt += 1;
        }
        // if virtual_edge_is_ok {
        //     write_board(&mut virtual_edge_ok, b)?;
        //     virtual_ok_cnt += 1;
        // } else {
        //     write_board(&mut virtual_edge_ng, b)?;
        //     virtual_ng_cnt += 1;
        // }

        // writeln!(
        //     summary,
        //     "{}, {}, {}",
        //     b.to_string(),
        //     if edge_is_ok { "ok" } else { "ng" },
        //     if virtual_edge_is_ok { "ok" } else { "ng" }
        // )?;
    }

    edge_ok.flush()?;
    edge_ng.flush()?;
    // virtual_edge_ok.flush()?;
    // virtual_edge_ng.flush()?;
    // summary.flush()?;

    println!(
        "edge done: {} boards (OK: {}, NG: {})",
        boards.len(),
        ok_cnt,
        ng_cnt
    );

    // println!(
    //     "virtual_edge done: {} boards (OK: {}, NG: {})",
    //     boards.len(),
    //     virtual_ok_cnt,
    //     virtual_ng_cnt
    // );
    Ok(())
}
