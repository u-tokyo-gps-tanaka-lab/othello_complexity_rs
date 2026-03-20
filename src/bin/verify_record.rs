use clap::Parser;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use othello_complexity_rs::io::parse_line_to_board;
use othello_complexity_rs::othello::{
    board_with_symmetry, coord_to_square, flip, get_moves, square_to_coord, Board,
};

#[derive(Parser, Debug)]
#[command(
    name = "verify-record",
    about = "Verify that a move record reaches a goal board from a start board"
)]
struct Cli {
    /// Use the standard initial position as start board
    #[arg(long)]
    from_initial: bool,

    /// Start board as a 64-cell X/O/- string (X = side to move)
    #[arg(long, value_name = "BOARD", allow_hyphen_values = true)]
    start: Option<String>,

    /// Goal board as a 64-cell X/O/- string
    #[arg(long, value_name = "BOARD", allow_hyphen_values = true)]
    goal: Option<String>,

    /// Move record string (e.g., "D3C3B3B2...")
    #[arg(long, value_name = "RECORD")]
    record: Option<String>,

    /// Input file with lines of "goal, record"
    #[arg(long, value_name = "FILE")]
    input_file: Option<PathBuf>,

    /// Check symmetric variants of goal
    #[arg(long, default_value_t = false)]
    check_symmetry: bool,
}

fn parse_record(record: &str) -> Result<Vec<usize>, String> {
    let record = record.trim();
    if record.is_empty() {
        return Err("empty record string".to_string());
    }
    if record.len() % 2 != 0 {
        return Err(format!(
            "record string length {} is not even: '{}'",
            record.len(),
            record
        ));
    }
    let mut moves = Vec::with_capacity(record.len() / 2);
    for i in (0..record.len()).step_by(2) {
        let coord = &record[i..i + 2];
        moves.push(coord_to_square(coord)?);
    }
    Ok(moves)
}

fn replay_record(start: Board, moves: &[usize]) -> Result<Board, String> {
    let mut board = start;
    for (move_idx, &sq) in moves.iter().enumerate() {
        let legal = get_moves(board.player, board.opponent);
        let current_board = if legal == 0 {
            // current player must pass
            let swapped = board.swapped();
            let opponent_legal = get_moves(swapped.player, swapped.opponent);
            if opponent_legal == 0 {
                return Err(format!(
                    "move {}: game already ended (both sides have no legal moves) but record has move {}",
                    move_idx + 1,
                    square_to_coord(sq)
                ));
            }
            swapped
        } else {
            board
        };

        let move_bb = 1_u64 << sq;
        let current_legal = get_moves(current_board.player, current_board.opponent);
        if (current_legal & move_bb) == 0 {
            return Err(format!(
                "move {}: {} is not a legal move\n  board: {}",
                move_idx + 1,
                square_to_coord(sq),
                current_board.to_string()
            ));
        }

        let flipped = flip(sq, current_board.player, current_board.opponent);
        if flipped == 0 {
            return Err(format!(
                "move {}: {} flips no discs",
                move_idx + 1,
                square_to_coord(sq)
            ));
        }

        board = Board::new(
            current_board.opponent ^ flipped,
            current_board.player ^ (flipped | move_bb),
        );
    }
    Ok(board)
}

fn board_matches_goal(final_board: Board, goal: Board, check_symmetry: bool) -> bool {
    if check_symmetry {
        for sym in 0..8_i32 {
            let goal_sym = board_with_symmetry(goal, sym);
            if final_board == goal_sym || final_board == goal_sym.swapped() {
                return true;
            }
        }
        false
    } else {
        final_board == goal || final_board == goal.swapped()
    }
}

fn verify_single(
    start: Board,
    goal: Board,
    record: &str,
    check_symmetry: bool,
) -> Result<(), String> {
    let moves = parse_record(record)?;
    let final_board = replay_record(start, &moves)?;

    if !board_matches_goal(final_board, goal, check_symmetry) {
        return Err(format!(
            "final board does not match goal\n  final:    {}\n  expected: {}",
            final_board.to_string(),
            goal.to_string()
        ));
    }
    Ok(())
}

fn resolve_start(cli: &Cli) -> Result<Board, String> {
    if cli.from_initial {
        if cli.start.is_some() {
            eprintln!("warning: --from-initial is set, so --start is ignored");
        }
        return Ok(Board::initial());
    }
    match cli.start.as_deref() {
        Some(raw) => parse_line_to_board(raw.trim())
            .ok_or_else(|| "invalid --start board: expected a 64-cell X/O/- string".to_string()),
        None => Err("start board is required: use --from-initial or --start".to_string()),
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let start = resolve_start(&cli)?;

    if let Some(path) = &cli.input_file {
        if cli.goal.is_some() || cli.record.is_some() {
            return Err("--input-file cannot be used with --goal or --record".to_string());
        }
        return run_file(start, path, cli.check_symmetry);
    }

    let goal_str = cli
        .goal
        .as_deref()
        .ok_or("--goal is required when not using --input-file")?;
    let record = cli
        .record
        .as_deref()
        .ok_or("--record is required when not using --input-file")?;

    let goal = parse_line_to_board(goal_str.trim())
        .ok_or_else(|| "invalid --goal board: expected a 64-cell X/O/- string".to_string())?;

    match verify_single(start, goal, record, cli.check_symmetry) {
        Ok(()) => {
            println!("OK: {}", goal_str.trim());
            Ok(())
        }
        Err(reason) => {
            println!("NG: {} -- {}", goal_str.trim(), reason);
            Err("verification failed".to_string())
        }
    }
}

fn run_file(start: Board, path: &PathBuf, check_symmetry: bool) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("failed to open {}: {}", path.display(), e))?;
    let reader = BufReader::new(file);

    let mut ok_count = 0_usize;
    let mut ng_count = 0_usize;

    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("read error at line {}: {}", line_no + 1, e))?;
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let (goal_str, record) = match line.split_once(',') {
            Some((g, r)) => (g.trim(), r.trim()),
            None => {
                println!("NG: line {} -- missing comma separator", line_no + 1);
                ng_count += 1;
                continue;
            }
        };

        let goal = match parse_line_to_board(goal_str) {
            Some(b) => b,
            None => {
                println!(
                    "NG: line {} -- invalid goal board: '{}'",
                    line_no + 1,
                    goal_str
                );
                ng_count += 1;
                continue;
            }
        };

        match verify_single(start, goal, record, check_symmetry) {
            Ok(()) => {
                println!("OK: {}", goal_str);
                ok_count += 1;
            }
            Err(reason) => {
                println!("NG: {} -- {}", goal_str, reason);
                ng_count += 1;
            }
        }
    }

    println!("\n--- Summary ---");
    println!("OK: {}", ok_count);
    println!("NG: {}", ng_count);
    println!("Total: {}", ok_count + ng_count);

    if ng_count > 0 {
        Err(format!("{} verification(s) failed", ng_count))
    } else {
        Ok(())
    }
}
