use std::collections::VecDeque;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::Parser;
use othello_complexity_rs::io::{format_position, parse_position};

use othello_complexity_rs::othello::{east, ne, north, nw, se, south, sw, west, CENTER_MASK};
use othello_complexity_rs::prunings::occupancy::{occupied_to_string, reachable_occupancy};

/// 中央4マスから到達可能なoccupied bitboardを計算し、各ステップの途中経過を返す
///
/// # 前提条件
/// - 中央2x2 (D4, E4, D5, E5) は常に占有されている必要がある
///
/// # 戻り値
/// - タプルの最初の要素: 中央4マスから到達可能なマス目を表すビットマスク（最終結果）
/// - タプルの2番目の要素: 中央からBFS順に外側へ広がるよう更新された`explained`の履歴（初期値を含む）
fn reachable_occupancy_with_steps(occupied: u64) -> io::Result<(u64, Vec<u64>)> {
    let final_explained = reachable_occupancy(occupied);
    let mut steps = Vec::new();
    let mut visited = CENTER_MASK & final_explained;

    // 初期状態（中央4マス）を記録
    steps.push(visited);

    if visited == final_explained {
        return Ok((final_explained, steps));
    }

    let mut queue = VecDeque::new();

    // 中央4マスからBFSの初期フロンティアを構築
    let mut seeds = visited;
    while seeds != 0 {
        let tz = seeds.trailing_zeros();
        let bit = 1u64 << tz;
        queue.push_back(bit);
        seeds &= seeds - 1;
    }

    // 8方向の近傍に順次拡張し、盤面中央から外側へと波状に広げる
    while let Some(bit) = queue.pop_front() {
        for neighbor in neighbors(bit) {
            if neighbor == 0 || (final_explained & neighbor) == 0 || (visited & neighbor) != 0 {
                continue;
            }
            visited |= neighbor;
            steps.push(visited);
            queue.push_back(neighbor);
        }
    }

    if visited != final_explained {
        return Err(io::Error::other(
            "occupancy traversal did not reach every explained square",
        ));
    }
    Ok((final_explained, steps))
}

/// 指定したマスの8近傍を返す（盤面外は0）
fn neighbors(bit: u64) -> [u64; 8] {
    [
        north(bit),
        ne(bit),
        east(bit),
        se(bit),
        south(bit),
        sw(bit),
        west(bit),
        nw(bit),
    ]
}

/// 診断用G/-形式の占有ビットマスクを読む。局面形式とは区別する。
/// - G: 占有マス (bit = 1)
/// - -: 非占有マス (bit = 0)
fn parse_mask(s: &str) -> Result<u64, String> {
    if s.len() != 64 {
        return Err(format!("Input must be 64 characters, got {}", s.len()));
    }

    let mut occupied = 0u64;
    for (i, ch) in s.chars().enumerate() {
        match ch {
            'G' => {
                occupied |= 1u64 << i;
            }
            '-' => {
                // non-occupied, do nothing
            }
            _ => {
                return Err(format!(
                    "Invalid character '{}' at position {}. Use G or -",
                    ch, i
                ));
            }
        }
    }
    if occupied & CENTER_MASK != CENTER_MASK {
        return Err("the centre four squares must be occupied".to_owned());
    }
    Ok(occupied)
}

/// stepsをファイルに書き込む（G/-形式の文字列のみ、1行に1ステップ）
fn write_steps_to_file(path: &Path, steps: &[u64]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    for &explained in steps {
        let line = occupied_to_string(explained);
        writeln!(file, "{}", line)?;
    }
    Ok(())
}

#[derive(Parser)]
#[command(about = "Visualize diagnostic occupancy masks; outputs are not position files")]
struct Cli {
    /// Diagnostic mask: exactly 64 G/- cells
    #[arg(
        long,
        allow_hyphen_values = true,
        conflicts_with = "position",
        required_unless_present = "position"
    )]
    mask: Option<String>,
    /// Absolute position; its correspondence with the masks is recorded
    #[arg(long, allow_hyphen_values = true)]
    position: Option<String>,
    #[arg(short = 'o', long)]
    out_dir: PathBuf,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();
    let (occupied, source_kind, source_value) = if let Some(text) = cli.position {
        let position = parse_position(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        (
            position.board.player | position.board.opponent,
            "position",
            format_position(position),
        )
    } else {
        let text = cli.mask.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "a mask or position is required",
            )
        })?;
        let mask = parse_mask(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        (mask, "mask", text)
    };
    let (explained, steps) = reachable_occupancy_with_steps(occupied)?;
    let steps_path = cli.out_dir.join(format!("0x{occupied:016x}_steps.txt"));
    let input_path = cli.out_dir.join(format!("0x{occupied:016x}_input.csv"));
    for path in [&steps_path, &input_path] {
        if path.try_exists()? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("output already exists: {}", path.display()),
            ));
        }
    }
    fs::create_dir_all(&cli.out_dir)?;
    write_steps_to_file(&steps_path, &steps)?;
    let mut input = csv::Writer::from_writer(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(input_path)?,
    );
    input.write_record(["source_kind", "source_value", "occupied_mask_G_dash"])?;
    input.write_record([source_kind, &source_value, &occupied_to_string(occupied)])?;
    input.flush()?;
    println!("Explained occupancy: {}", occupied_to_string(explained));
    println!("Diagnostic steps: {}", steps_path.display());
    Ok(())
}
