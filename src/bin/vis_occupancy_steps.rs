use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::Path;

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
fn reachable_occupancy_with_steps(occupied: u64) -> (u64, Vec<u64>) {
    let final_explained = reachable_occupancy(occupied);
    let mut steps = Vec::new();
    let mut visited = CENTER_MASK & final_explained;

    // 初期状態（中央4マス）を記録
    steps.push(visited);

    if visited == final_explained {
        return (final_explained, steps);
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

    // 念のため、BFSで拾えなかったマスがあれば補完（理論上は空のはず）
    if visited != final_explained {
        eprint!("warning: some squares were not reached in BFS, completing remaining squares...\n");
        let mut remaining = final_explained & !visited;
        while remaining != 0 {
            let tz = remaining.trailing_zeros();
            let bit = 1u64 << tz;
            visited |= bit;
            steps.push(visited);
            remaining &= remaining - 1;
        }
    }

    (final_explained, steps)
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

/// stepsをファイルに書き込む（G/-形式の文字列のみ、1行に1ステップ）
fn write_steps_to_file(path: &Path, steps: &[u64]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    for &explained in steps {
        let line = occupied_to_string(explained);
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
        let line = occupied_to_string(explained);
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
