//! Uniform sampling of absolute positions or geometric/color equivalence classes.
//!
//! An absolute rank's low bit is the turn (0 = Black, 1 = White). The remaining
//! digits run from A1 to H8, in radix 2 at the four occupied central squares and
//! radix 3 elsewhere. Central digits 0/1 mean X/O; other digits 0/1/2 mean -/X/O.
//! Changing only the turn bit exchanges absolute colors as well as the turn.
//! Class representatives have Black to move and minimize (occupancy, player)
//! among the eight geometric images. Sampling rejects nonrepresentatives instead
//! of mapping proposals to their representatives, which would favor larger orbits.

use crate::io::{format_position, parse_file_to_positions};
use crate::othello::{validate_board, Board, BoardSymmetry, Color, Position, CENTER_MASK};
use rand::{seq::SliceRandom, Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

pub const POPULATION_SIZE: u128 = 32 * 3_u128.pow(60);
pub const POPULATION_DEFINITION: &str = "absolute_positions_center_occupied";
pub const DEFAULT_SEED: u64 = 42; // Default random seed for reproducible sampling.
pub const CLASS_POPULATION_SIZE: u128 =
    (16 * 3_u128.pow(60) + 4 * 3_u128.pow(15) + 12 * 3_u128.pow(30) + 16 * 3_u128.pow(33)) / 8;

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize, clap::ValueEnum)]
pub enum Population {
    #[value(name = "exact")]
    #[serde(rename = "exact")]
    Absolute,
    #[value(name = "equivalence")]
    #[serde(rename = "equivalence")]
    Classes,
}

impl Population {
    pub fn size(self, discs: Option<u32>) -> Result<u128, String> {
        match (self, discs) {
            (Self::Absolute, None) => Ok(POPULATION_SIZE),
            (Self::Absolute, Some(discs)) => fixed_discs_population(discs),
            (Self::Classes, None) => Ok(CLASS_POPULATION_SIZE),
            (Self::Classes, Some(discs)) => fixed_discs_class_population(discs),
        }
    }

    pub fn metadata(self, discs: Option<u32>) -> Result<Value, String> {
        let definition = match (self, discs) {
            (Self::Absolute, None) => POPULATION_DEFINITION,
            (Self::Absolute, Some(_)) => "absolute_positions_fixed_discs",
            (Self::Classes, None) => "equivalence_classes_center_occupied",
            (Self::Classes, Some(_)) => "equivalence_classes_fixed_discs",
        };
        let mut metadata = json!({"definition": definition, "size": self.size(discs)?.to_string()});
        if let Some(discs) = discs {
            metadata["discs"] = json!(discs);
        }
        Ok(metadata)
    }

    /// Draw one sample with replacement from the selected population.
    /// For classes, each accepted representative has exactly one equally likely
    /// Black-to-move proposal, regardless of its number of distinct images.
    pub fn sample(self, rng: &mut impl Rng, discs: Option<u32>) -> Result<Position, String> {
        loop {
            let position = match (self, discs) {
                (Self::Absolute, None) => position_from_rank(rng.random_range(0..POPULATION_SIZE))?,
                (Self::Absolute, Some(discs)) => sample_fixed_discs(rng, discs)?,
                (Self::Classes, None) => {
                    position_from_rank(2 * rng.random_range(0..POPULATION_SIZE / 2))?
                }
                (Self::Classes, Some(discs)) => Position {
                    board: sample_fixed_discs_board(rng, discs)?,
                    side_to_move: Color::Black,
                },
            };
            if self == Self::Absolute || is_representative(position.board) {
                return Ok(position);
            }
        }
    }
}

/// Validate sampling settings without touching the output directory.
pub fn validate_generation(
    population: Population,
    discs: Option<u32>,
    count: usize,
) -> io::Result<()> {
    if count == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "count must be positive",
        ));
    }
    population
        .metadata(discs)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    Ok(())
}

/// Generate a uniform sample and its metadata in a new or reusable directory.
pub fn generate(
    out_dir: &Path,
    population: Population,
    discs: Option<u32>,
    count: usize,
    seed: u64,
) -> io::Result<()> {
    validate_generation(population, discs, count)?;
    let population_metadata = population
        .metadata(discs)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    create_output_dir(out_dir, "sampling.json")?;
    let positions_path = out_dir.join("positions.txt");
    let mapping_path = out_dir.join("mapping.jsonl");
    let mut output = BufWriter::new(File::create(&positions_path)?);
    let mut mapping = BufWriter::new(File::create(&mapping_path)?);
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    for index in 1..=count {
        let position = population
            .sample(&mut rng, discs)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        writeln!(output, "{}", format_position(position))?;
        serde_json::to_writer(
            &mut mapping,
            &json!({
                "input_line": index, "output_line": index, "accepted": true,
                "position": format_position(position), "absolute_rank": rank_of_position(position)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
                    .to_string(),
            }),
        )?;
        writeln!(mapping)?;
    }
    output.flush()?;
    mapping.flush()?;
    write_sampling_record(
        out_dir,
        json!({
            "method": if discs.is_some() { "fixed-discs" } else { "uniform" },
            "population": population_metadata, "uniform": true, "with_replacement": true,
            "seed": seed, "input": null, "input_count": count,
            "accepted_count": count, "output": artifact(&positions_path)?,
            "mapping": artifact(&mapping_path)?,
        }),
    )?;
    Ok(())
}

/// Validated sampling metadata and observations, preserving their multiplicity.
pub struct Sample {
    pub population: Population,
    pub discs: Option<u32>,
    pub positions: Vec<Position>,
    pub metadata: Value,
}

/// Read and validate positions.txt and sampling.json for population estimation.
pub fn read_sample_dir(directory: &Path) -> io::Result<Sample> {
    let invalid = |message| io::Error::new(io::ErrorKind::InvalidData, message);
    let metadata: Value = serde_json::from_reader(File::open(directory.join("sampling.json"))?)?;
    if metadata["schema_version"].as_u64() != Some(1) {
        return Err(invalid(
            "unsupported sampling schema; expected schema_version 1",
        ));
    }
    if metadata["uniform"].as_bool() != Some(true) {
        return Err(invalid("nonuniform input is not an estimation sample"));
    }
    let definition = metadata["population"]["definition"]
        .as_str()
        .ok_or_else(|| invalid("sampling population must specify a definition"))?;
    let (population, fixed_discs) = match definition {
        POPULATION_DEFINITION => (Population::Absolute, false),
        "absolute_positions_fixed_discs" => (Population::Absolute, true),
        "equivalence_classes_center_occupied" => (Population::Classes, false),
        "equivalence_classes_fixed_discs" => (Population::Classes, true),
        _ => return Err(invalid("unsupported sampling population")),
    };
    let discs = if fixed_discs {
        Some(
            metadata["population"]["discs"]
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| invalid("population discs must be an integer in 4..=64"))?,
        )
    } else {
        None
    };
    let expected_population = population
        .metadata(discs)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if metadata["population"] != expected_population {
        return Err(invalid(
            "sampling population size or fields are inconsistent",
        ));
    }
    let method = metadata["method"]
        .as_str()
        .ok_or_else(|| invalid("sampling record must specify a method"))?;
    let converted = matches!(method, "raw" | "representatives");
    let expected_method = if fixed_discs {
        "fixed-discs"
    } else {
        "uniform"
    };
    if method != expected_method
        && !(converted && population == Population::Absolute && !fixed_discs)
    {
        return Err(invalid("sampling method does not match its population"));
    }
    let with_replacement = metadata["with_replacement"].as_bool().ok_or_else(|| {
        invalid("sampling record must state whether sampling is with replacement")
    })?;
    if metadata["seed"].as_u64().is_none() {
        return Err(invalid("sampling seed must be a u64 integer"));
    }
    if converted {
        let provenance = metadata["provenance"]["path"].as_str().ok_or_else(|| {
            invalid("converted sampling record must identify its provenance file")
        })?;
        let provenance = Path::new(provenance);
        if !provenance.is_absolute() {
            return Err(invalid("sampling provenance path must be absolute"));
        }
        if !fs::metadata(provenance)?.is_file() {
            return Err(invalid("sampling provenance must be a file"));
        }
    } else if !with_replacement {
        return Err(invalid("uniform generation uses replacement"));
    }
    let positions_path = directory.join("positions.txt");
    let positions = parse_file_to_positions(
        positions_path
            .to_str()
            .ok_or_else(|| invalid("sample positions path must be valid UTF-8"))?,
    )?;
    if metadata["accepted_count"].as_u64() != Some(positions.len() as u64) {
        return Err(invalid(
            "sampling accepted_count differs from positions.txt row count",
        ));
    }
    if positions.is_empty() {
        return Err(invalid("cannot estimate from an empty sample"));
    }
    for (index, position) in positions.iter().enumerate() {
        let occupied = position.board.player | position.board.opponent;
        if discs.is_some_and(|discs| occupied.count_ones() != discs) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "sample row {}: stone count differs from its population",
                    index + 1
                ),
            ));
        }
        if population == Population::Classes
            && (position.side_to_move != Color::Black || !is_representative(position.board))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "sample row {}: expected the black-turn (occupancy, player) representative",
                    index + 1
                ),
            ));
        }
    }
    Ok(Sample {
        population,
        discs,
        positions,
        metadata,
    })
}

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

/// Number of absolute positions having exactly `discs` occupied squares.
pub fn fixed_discs_population(discs: u32) -> Result<u128, String> {
    if !(4..=64).contains(&discs) {
        return Err("discs must be in 4..=64".into());
    }
    let k = (discs - 4).min(64 - discs);
    let mut combinations = 1_u128;
    for i in 1..=k {
        combinations = combinations * u128::from(61 - i) / u128::from(i);
    }
    Ok(combinations * 2_u128.pow(discs + 1))
}

/// Burnside's lemma on Black-to-move boards. Each central cycle has two
/// constant colors; a noncentral cycle of length l contributes 1 + 2*z^l.
pub fn fixed_discs_class_population(discs: u32) -> Result<u128, String> {
    if !(4..=64).contains(&discs) {
        return Err("discs must be in 4..=64".into());
    }
    let mut fixed_sum = 0_u128;
    for symmetry in BoardSymmetry::ALL {
        let mut seen = 0_u64;
        let mut central_cycles = 0;
        let mut coefficients = [0_u128; 61];
        coefficients[0] = 1;
        for square in 0..64 {
            let mut bit = 1_u64 << square;
            if seen & bit != 0 {
                continue;
            }
            let central = bit & CENTER_MASK != 0;
            let mut length = 0;
            while seen & bit == 0 {
                seen |= bit;
                length += 1;
                bit = symmetry.apply(Board::new(bit, 0)).player;
            }
            if central {
                central_cycles += 1;
            } else {
                for occupied in (length..=60).rev() {
                    coefficients[occupied] += 2 * coefficients[occupied - length];
                }
            }
        }
        fixed_sum += 2_u128.pow(central_cycles) * coefficients[(discs - 4) as usize];
    }
    Ok(fixed_sum / 8)
}

/// Uniform subset of noncentral squares, then independent fair colors and turn.
pub fn sample_fixed_discs(rng: &mut impl Rng, discs: u32) -> Result<Position, String> {
    Ok(Position {
        board: sample_fixed_discs_board(rng, discs)?,
        side_to_move: if rng.random::<bool>() {
            Color::White
        } else {
            Color::Black
        },
    })
}

fn sample_fixed_discs_board(rng: &mut impl Rng, discs: u32) -> Result<Board, String> {
    fixed_discs_population(discs)?;
    let mut squares: Vec<_> = (0..64)
        .filter(|square| (1_u64 << square) & CENTER_MASK == 0)
        .collect();
    squares.shuffle(rng);
    let mut occupied = CENTER_MASK;
    for &square in &squares[..(discs - 4) as usize] {
        occupied |= 1_u64 << square;
    }
    let player = rng.random::<u64>() & occupied;
    Ok(Board::new(player, occupied & !player))
}

/// Select the minimum (occupancy, player) within each geometric orbit.
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

/// Prepare an output directory and remove its previous completion record.
/// Existing directories are reused, and all other files are left unchanged.
pub fn create_output_dir(path: &Path, record: &str) -> io::Result<()> {
    fs::create_dir_all(path)?;
    match fs::remove_file(path.join(record)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
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
    let mut file = File::create(directory.join("sampling.json"))?;
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
