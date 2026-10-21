//! Absolute-position sampling, with multiplicities and an explicit sampling design.
//!
//! A rank's low bit is the absolute turn (0 = Black, 1 = White). The remaining
//! digits run from A1 to H8, in radix 2 at the four occupied central squares and
//! radix 3 elsewhere. Central digits 0/1 mean X/O; other digits 0/1/2 mean -/X/O.
//! Changing only the turn bit exchanges absolute colors as well as the turn.
//! No symmetry quotient or reachability condition enters this population.

use crate::othello::{validate_board, Board, BoardSymmetry, Color, Position, CENTER_MASK};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

pub const POPULATION_SIZE: u128 = 32 * 3_u128.pow(60);
pub const POPULATION_DEFINITION: &str = "absolute_positions_center_occupied";

/// Decode one rank, rejecting values outside the entire absolute population.
pub fn position_from_rank(mut rank: u128) -> Result<Position, String> {
    if rank >= POPULATION_SIZE {
        return Err(format!("rank must be less than {POPULATION_SIZE}"));
    }
    let side_to_move = if rank & 1 == 0 {
        Color::Black
    } else {
        Color::White
    };
    rank /= 2;
    let mut board = Board::empty();
    for square in 0..64 {
        let mask = 1_u64 << square;
        let central = mask & CENTER_MASK != 0;
        let radix = if central { 2 } else { 3 };
        let digit = rank % radix;
        rank /= radix;
        if digit == u128::from(!central) {
            board.player |= mask;
        } else if digit == radix - 1 {
            board.opponent |= mask;
        }
    }
    Ok(Position {
        board,
        side_to_move,
    })
}

/// Inverse of [`position_from_rank`], including both absolute turns.
pub fn rank_of_position(position: Position) -> Result<u128, String> {
    validate_board(&position.board).map_err(|error| format!("invalid board: {error:?}"))?;
    let mut rank = u128::from(position.side_to_move == Color::White);
    let mut weight = 2;
    for square in 0..64 {
        let mask = 1_u64 << square;
        let central = mask & CENTER_MASK != 0;
        let (radix, digit) = if central {
            (2, u128::from(position.board.opponent & mask != 0))
        } else {
            (
                3,
                if position.board.player & mask != 0 {
                    1
                } else if position.board.opponent & mask != 0 {
                    2
                } else {
                    0
                },
            )
        };
        rank += weight * digit;
        weight *= radix;
    }
    Ok(rank)
}

/// The retained representatives were selected by (occupancy, player), not by
/// canonicalizing every sampled board. Only this predicate defines that sample.
pub fn is_representative(board: Board) -> bool {
    let key = (board.player | board.opponent, board.player);
    BoardSymmetry::ALL.into_iter().all(|symmetry| {
        let image = symmetry.apply(board);
        (image.player | image.opponent, image.player) >= key
    })
}

/// Deduplicated geometric images with Black then White for each image.
/// The first symmetry producing a position labels it in the conversion map.
pub fn representative_candidates(board: Board) -> Vec<(BoardSymmetry, Position)> {
    let mut candidates = Vec::new();
    for symmetry in BoardSymmetry::ALL {
        for side_to_move in [Color::Black, Color::White] {
            let position = Position {
                board: symmetry.apply(board),
                side_to_move,
            };
            if !candidates.iter().any(|(_, previous)| *previous == position) {
                candidates.push((symmetry, position));
            }
        }
    }
    candidates
}

/// One uniform integer in 0..16 implements both rejection and selection:
/// m accepted integers give probability m/16, and each candidate gets 1/16.
pub fn select_representative(
    candidates: &[(BoardSymmetry, Position)],
    draw: usize,
) -> Result<Option<(BoardSymmetry, Position)>, String> {
    if draw >= 16 || candidates.is_empty() || candidates.len() > 16 {
        return Err("invalid representative candidates or draw".into());
    }
    Ok(candidates.get(draw).copied())
}

pub fn population() -> Value {
    json!({"definition": POPULATION_DEFINITION, "size": POPULATION_SIZE.to_string()})
}

/// SHA-256 of exact bytes, including line endings.
pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn artifact(path: &Path) -> io::Result<Value> {
    Ok(json!({
        "path": fs::canonicalize(path)?,
        "sha256": sha256(&fs::read(path)?),
    }))
}

/// Make a new experiment directory; never overwrite or reuse an existing one.
pub fn create_output_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(path)
}

/// Write a completed sampling record only after its referenced artifacts exist.
/// The lockfile digest pins all dependency versions; RNG versions are also
/// recorded explicitly because seeds alone do not specify a sampling stream.
pub fn write_sampling_record(directory: &Path, mut record: Value) -> io::Result<()> {
    record["schema_version"] = json!(1);
    record["software"] = json!({
        "package": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
        "cargo_lock_sha256": sha256(include_bytes!("../Cargo.lock")),
        "rng": "rand_chacha::ChaCha8Rng",
        "rand_chacha_version": lock_version("rand_chacha")?,
        "rand_version": lock_version("rand")?,
    });
    let mut file = File::create_new(directory.join("sampling.json"))?;
    serde_json::to_writer_pretty(&mut file, &record)?;
    writeln!(file)?;
    file.sync_all()
}

fn lock_version(name: &str) -> io::Result<String> {
    // The sampling dependencies are both in the 0.9 series. Older rand and
    // rand_chacha releases also occur transitively and must not be reported.
    for package in include_str!("../Cargo.lock").split("[[package]]") {
        if package
            .lines()
            .any(|line| line == format!("name = \"{name}\""))
        {
            if let Some(version) = package.lines().find_map(|line| {
                line.strip_prefix("version = \"")
                    .and_then(|s| s.strip_suffix('"'))
            }) {
                if version.starts_with("0.9.") {
                    return Ok(version.to_owned());
                }
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "dependency version missing from Cargo.lock",
    ))
}
