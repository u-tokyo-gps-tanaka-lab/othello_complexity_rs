use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::Path;

use othello_complexity_rs::io::parse_position;
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

/// 診断用G/-形式の文字列をu64ビットボードに変換
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
        eprintln!("Usage: {} <position> [output_dir]", args[0]);
        eprintln!("       {} --mask <mask_string> [output_dir]", args[0]);
        eprintln!("  <position>: 64 X/O/- cells, one space, and b or w");
        eprintln!("  <mask_string>: 64 G/- cells (G = occupied, - = empty)");
        eprintln!("  [output_dir]: Optional output directory (default: current directory)");
        eprintln!();
        eprintln!("  Output format: G/- (G = reachable, - = unreachable)");
        eprintln!("  Output file: <occupied_hex>_steps.txt");
        eprintln!();
        eprintln!(
            "Example: {} --mask \"-------------------GG-----GGGG----GGGG-----GG-------------------\"",
            args[0]
        );
        eprintln!(
            "Example: {} --mask \"-------------------GG-----GGGG----GGGG-----GG-------------------\" output/",
            args[0]
        );
        std::process::exit(1);
    }

    // 局面と診断用マスクを明示的に区別してoccupiedを取得する。
    let mask_input = args[1] == "--mask";
    let input_index = if mask_input { 2 } else { 1 };
    let Some(board_str) = args.get(input_index) else {
        eprintln!("--mask requires a 64-character G/- string");
        std::process::exit(1);
    };
    let parsed = if mask_input {
        parse_mask(board_str)
    } else {
        parse_position(board_str).map(|position| position.board.player | position.board.opponent)
    };
    let occupied = match parsed {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error parsing board: {}", e);
            std::process::exit(1);
        }
    };

    // 出力ディレクトリを取得（指定がなければ現在のディレクトリ）
    let output_dir = if args.len() > input_index + 1 {
        &args[input_index + 1]
    } else {
        "."
    };

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
