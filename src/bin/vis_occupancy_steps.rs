use std::fs;
use std::io::Write;
use std::path::Path;

use othello_complexity_rs::othello::CENTER_MASK;
use othello_complexity_rs::prunings::occupancy::{backshift, Dir};

/// 中央4マスから到達可能なoccupied bitboardを計算し、各ステップの途中経過を返す
///
/// # 前提条件
/// - 中央2x2 (D4, E4, D5, E5) は常に占有されている必要がある
///
/// # 戻り値
/// - タプルの最初の要素: 中央4マスから到達可能なマス目を表すビットマスク（最終結果）
/// - タプルの2番目の要素: 各反復ステップでの`explained`の値を記録したVec（初期値を含む）
fn reachable_occupancy_with_steps(occupied: u64) -> (u64, Vec<u64>) {
    let dirs = [
        Dir::N,
        Dir::S,
        Dir::E,
        Dir::W,
        Dir::NE,
        Dir::NW,
        Dir::SE,
        Dir::SW,
    ];
    let mut explained: u64 = CENTER_MASK;
    let mut steps = vec![explained];
    for _ in 0..60 {
        let mut add_all: u64 = 0;
        for &d in &dirs {
            let w1 = backshift(d, explained) & explained;
            let mut scanning_pos = backshift(d, w1) & occupied;
            let mut r_d = scanning_pos;
            while scanning_pos != 0 {
                scanning_pos = backshift(d, scanning_pos) & occupied;
                r_d |= scanning_pos;
            }
            add_all |= r_d;
        }
        let add = add_all & !explained;
        if add == 0 {
            break;
        }
        explained |= add;
        steps.push(explained); // 各ステップを記録
        if explained == occupied {
            return (explained, steps);
        }
    }
    (explained, steps)
}

/// O/X/G/-形式の文字列をu64ビットボードに変換
/// - O, X, または G: 占有マス (bit = 1)
/// - -: 非占有マス (bit = 0)
fn parse_board(s: &str) -> Result<u64, String> {
    if s.len() != 64 {
        return Err(format!("Input must be 64 characters, got {}", s.len()));
    }

    let mut occupied = 0u64;
    for (i, ch) in s.chars().enumerate() {
        match ch {
            'O' | 'X' | 'G' => {
                occupied |= 1u64 << i;
            }
            '-' => {
                // non-occupied, do nothing
            }
            _ => {
                return Err(format!(
                    "Invalid character '{}' at position {}. Use O, X, G, or -",
                    ch, i
                ));
            }
        }
    }
    Ok(occupied)
}

/// explainedをG/-形式の文字列を生成
/// - G: explained (到達可能なマス)
/// - -: not explained (到達不可能なマス)
fn format_step(explained: u64) -> String {
    let mut s = String::new();
    for y in 0..8 {
        for x in 0..8 {
            let i = y * 8 + x;
            let bit = 1u64 << i;
            if explained & bit != 0 {
                s.push('G');
            } else {
                s.push('-');
            }
        }
    }
    s
}

/// stepsをファイルに書き込む（G/-形式の文字列のみ、1行に1ステップ）
fn write_steps_to_file(path: &Path, steps: &[u64]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    for &explained in steps {
        let line = format_step(explained);
        writeln!(file, "{}", line)?;
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <board_string> [output_dir]", args[0]);
        eprintln!("  <board_string>: 64-character string using O/X/G/- format");
        eprintln!("    O, X, or G: occupied square");
        eprintln!("    -: empty square");
        eprintln!("  [output_dir]: Optional output directory (default: current directory)");
        eprintln!();
        eprintln!("  Output format: G/- (G = reachable, - = unreachable)");
        eprintln!("  Output file: <occupied_hex>_steps.txt");
        eprintln!();
        eprintln!(
            "Example: {} \"-------------------GG-----GGGG----GGGG-----GG-------------------\"",
            args[0]
        );
        eprintln!(
            "Example: {} \"-------------------GG-----GGGG----GGGG-----GG-------------------\" output/",
            args[0]
        );
        std::process::exit(1);
    }

    // コマンドライン引数からoccupiedを取得（O/X/G/-形式文字列）
    let board_str = &args[1];
    let occupied = match parse_board(board_str) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error parsing board: {}", e);
            std::process::exit(1);
        }
    };

    // 出力ディレクトリを取得（指定がなければ現在のディレクトリ）
    let output_dir = if args.len() >= 3 { &args[2] } else { "." };

    println!("Input:    {}", board_str);
    println!("Occupied: 0x{:016x}", occupied);
    println!();

    let (final_result, steps) = reachable_occupancy_with_steps(occupied);

    // 標準出力に表示
    for (i, &explained) in steps.iter().enumerate() {
        let line = format_step(explained);
        println!("Step {}: {}", i, line);
    }

    println!();
    println!("Final result: 0x{:016x}", final_result);
    println!(
        "All squares reachable: {}",
        if final_result == occupied {
            "Yes"
        } else {
            "No"
        }
    );

    // ファイル名を生成: 0x{occupied:016x}_steps.txt
    let filename = format!("0x{:016x}_steps.txt", occupied);
    let output_path = Path::new(output_dir).join(&filename);

    // ファイルに書き込み
    match write_steps_to_file(&output_path, &steps) {
        Ok(_) => {
            println!();
            println!("Steps written to: {}", output_path.display());
        }
        Err(e) => {
            eprintln!();
            eprintln!("Error writing to file: {}", e);
            std::process::exit(1);
        }
    }
}
