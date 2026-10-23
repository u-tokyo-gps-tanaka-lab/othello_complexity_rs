use clap::Parser;
use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{get_moves, Position};
use othello_complexity_rs::record::{
    replay, GoalMatch, PlyAction, PositionTransform, ReachabilityRecord,
};
use othello_complexity_rs::sampling::{
    artifact, create_output_dir, population, write_sampling_record,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde_json::json;
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Generate legal games with explicit passes and verified absolute endpoints")]
struct Args {
    /// Maximum actions, counting passes; omit to play until the game ends
    #[arg(long, value_parser = clap::value_parser!(u32).range(0..=120))]
    plies: Option<u32>,
    #[arg(short = 'c', long)]
    count: usize,
    #[arg(long)]
    seed: u64,
    #[arg(short = 'o', long)]
    out_dir: PathBuf,
}
fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    if args.count == 0 {
        return Err("count must be positive".into());
    }
    create_output_dir(&args.out_dir)?;
    let positions_path = args.out_dir.join("positions.txt");
    let records_path = args.out_dir.join("records.csv");
    let mut positions = BufWriter::new(File::create_new(&positions_path)?);
    let mut records = BufWriter::new(File::create_new(&records_path)?);
    let mut rng = ChaCha8Rng::seed_from_u64(args.seed);
    for input_index in 1..=args.count {
        let mut current = Position::initial();
        let mut actions = Vec::new();
        while !args
            .plies
            .is_some_and(|limit| actions.len() >= limit as usize)
        {
            let mut moves = get_moves(current.board.player, current.board.opponent);
            let action = if moves == 0 {
                if get_moves(current.board.opponent, current.board.player) == 0 {
                    break;
                }
                PlyAction::Pass
            } else {
                for _ in 0..rng.random_range(0..moves.count_ones()) {
                    moves &= moves - 1;
                }
                PlyAction::Play(moves.trailing_zeros() as usize)
            };
            current = replay(current, &[action])?[1];
            actions.push(action);
        }
        let record = ReachabilityRecord {
            input_index,
            start: Position::initial(),
            goal: current,
            goal_match: GoalMatch::Exact,
            transform: PositionTransform::IDENTITY,
            actions,
        };
        record.verify()?;
        writeln!(positions, "{}", format_position(current))?;
        writeln!(records, "{}", record.to_csv())?;
    }
    positions.flush()?;
    records.flush()?;
    write_sampling_record(
        &args.out_dir,
        json!({
            "method": "random-play", "population": population(), "uniform": false,
            "with_replacement": null, "seed": args.seed, "plies": args.plies,
            "accepted_count": args.count, "output": artifact(&positions_path)?, "records": artifact(&records_path)?,
        }),
    )?;
    Ok(())
}
