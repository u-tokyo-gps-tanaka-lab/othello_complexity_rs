//! Small, validated CPU rankers. Scores are never reachability decisions.
//!
//! Inputs describe the exact position, including absolute side to move. Models
//! are trained for the standard initial position and only order search work.

use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::othello::{get_last_moves, get_moves, Color, Position, CENTER_MASK};

pub const MODEL_SCHEMA: &str = "othello-reverse-ranker-v1";
pub const FEATURE_VERSION: &str = "reverse-board-v1";
pub const FEATURE_COUNT: usize = 160;
const MAX_HIDDEN: usize = 128;
const MAX_ABS_PARAMETER: f64 = 1_000_000.0;
const MAX_MODEL_BYTES: usize = 8 * 1024 * 1024;
const NOT_A: u64 = 0xfefe_fefe_fefe_fefe;
const NOT_H: u64 = 0x7f7f_7f7f_7f7f_7f7f;
const CORNERS: u64 = 0x8100_0000_0000_0081;
const EDGES: u64 = 0xff81_8181_8181_81ff;

/// Indices 0..64 and 64..128 are the relative player/opponent bit planes in
/// increasing bit order. The following names document the remaining inputs.
pub const AGGREGATE_FEATURE_NAMES: [&str; 32] = [
    "black_to_move",
    "occupied_over_64",
    "player_over_64",
    "opponent_over_64",
    "player_moves_over_64",
    "opponent_moves_over_64",
    "last_move_squares_over_64",
    "legal_reverse_pass",
    "player_frontier_over_64",
    "opponent_frontier_over_64",
    "player_potential_moves_over_64",
    "opponent_potential_moves_over_64",
    "player_corners_over_4",
    "opponent_corners_over_4",
    "player_edges_over_28",
    "opponent_edges_over_28",
    "player_center_over_4",
    "opponent_center_over_4",
    "player_without_same_color_neighbor_over_64",
    "opponent_without_same_color_neighbor_over_64",
    "player_horizontal_pairs_over_56",
    "opponent_horizontal_pairs_over_56",
    "player_vertical_pairs_over_56",
    "opponent_vertical_pairs_over_56",
    "player_diagonal_7_pairs_over_49",
    "opponent_diagonal_7_pairs_over_49",
    "player_diagonal_9_pairs_over_49",
    "opponent_diagonal_9_pairs_over_49",
    "player_touching_opponent_over_64",
    "opponent_touching_player_over_64",
    "empty_touching_occupied_over_64",
    "absolute_disc_imbalance_over_64",
];

fn neighbors(bits: u64) -> u64 {
    let horizontal = ((bits & NOT_H) << 1) | ((bits & NOT_A) >> 1);
    horizontal | (bits << 8) | (bits >> 8) | (horizontal << 8) | (horizontal >> 8)
}

/// Bounded [0,1] features; no fitted preprocessing or hidden model state.
pub fn features(position: Position) -> [f64; FEATURE_COUNT] {
    let p = position.board.player;
    let o = position.board.opponent;
    let occupied = p | o;
    let empty = !occupied;
    let np = neighbors(p);
    let no = neighbors(o);
    let ne = neighbors(empty);
    let mp = get_moves(p, o);
    let mo = get_moves(o, p);
    let count = |bits: u64, denominator: f64| f64::from(bits.count_ones()) / denominator;
    let mut values = [0.0; FEATURE_COUNT];
    for square in 0..64 {
        values[square] = ((p >> square) & 1) as f64;
        values[64 + square] = ((o >> square) & 1) as f64;
    }
    values[128..].copy_from_slice(&[
        f64::from(position.side_to_move == Color::Black),
        count(occupied, 64.0),
        count(p, 64.0),
        count(o, 64.0),
        count(mp, 64.0),
        count(mo, 64.0),
        count(get_last_moves(o, p), 64.0),
        f64::from(mo == 0 && mp != 0),
        count(p & ne, 64.0),
        count(o & ne, 64.0),
        count(empty & no, 64.0),
        count(empty & np, 64.0),
        count(p & CORNERS, 4.0),
        count(o & CORNERS, 4.0),
        count(p & EDGES, 28.0),
        count(o & EDGES, 28.0),
        count(p & CENTER_MASK, 4.0),
        count(o & CENTER_MASK, 4.0),
        count(p & !np, 64.0),
        count(o & !no, 64.0),
        count(p & ((p & NOT_H) << 1), 56.0),
        count(o & ((o & NOT_H) << 1), 56.0),
        count(p & (p << 8), 56.0),
        count(o & (o << 8), 56.0),
        count(p & ((p & NOT_A) << 7), 49.0),
        count(o & ((o & NOT_A) << 7), 49.0),
        count(p & ((p & NOT_H) << 9), 49.0),
        count(o & ((o & NOT_H) << 9), 49.0),
        count(p & no, 64.0),
        count(o & np, 64.0),
        count(empty & (np | no), 64.0),
        f64::from(p.count_ones().abs_diff(o.count_ones())) / 64.0,
    ]);
    values
}

/// A linear ranker or one-hidden-layer ReLU network. Parameters are private,
/// shape checked and bounded. Even the maximum accepted parameter magnitudes
/// cannot overflow f64 on the bounded feature domain.
#[derive(Clone, Debug)]
pub struct LearnedModel {
    hidden_size: usize,
    input_weights: Vec<f64>,
    hidden_bias: Vec<f64>,
    output_weights: Vec<f64>,
    output_bias: f64,
    sha256: String,
}

fn invalid(message: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn parameter(value: &Value, name: &str) -> io::Result<f64> {
    let number = value
        .as_f64()
        .ok_or_else(|| invalid(format!("{name} must be a finite number")))?;
    if !number.is_finite() || number.abs() > MAX_ABS_PARAMETER {
        return Err(invalid(format!(
            "{name} must be finite with absolute value at most {MAX_ABS_PARAMETER}"
        )));
    }
    Ok(number)
}

fn parameters(root: &Value, name: &str, length: usize) -> io::Result<Vec<f64>> {
    let values = root[name]
        .as_array()
        .ok_or_else(|| invalid(format!("{name} must be an array")))?;
    if values.len() != length {
        return Err(invalid(format!("{name} must have length {length}")));
    }
    values.iter().map(|value| parameter(value, name)).collect()
}

impl LearnedModel {
    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = fs::read(path)?;
        if bytes.len() > MAX_MODEL_BYTES {
            return Err(invalid("model exceeds the 8 MiB size limit"));
        }
        let contents = std::str::from_utf8(&bytes).map_err(invalid)?;
        Self::from_json(contents)
    }

    pub fn from_json(contents: &str) -> io::Result<Self> {
        if contents.len() > MAX_MODEL_BYTES {
            return Err(invalid("model exceeds the 8 MiB size limit"));
        }
        let root: Value = serde_json::from_str(contents).map_err(invalid)?;
        if root["schema"].as_str() != Some(MODEL_SCHEMA)
            || root["feature_version"].as_str() != Some(FEATURE_VERSION)
            || root["feature_count"].as_u64() != Some(FEATURE_COUNT as u64)
        {
            return Err(invalid("unsupported ranker schema or feature definition"));
        }
        let hidden_size = root["hidden_size"]
            .as_u64()
            .filter(|&size| size <= MAX_HIDDEN as u64)
            .ok_or_else(|| invalid("hidden_size must be an integer from 0 through 128"))?
            as usize;
        let input_weights = parameters(&root, "input_weights", FEATURE_COUNT * hidden_size.max(1))?;
        let hidden_bias = parameters(&root, "hidden_bias", hidden_size)?;
        let output_weights = parameters(&root, "output_weights", hidden_size)?;
        let output_bias = parameter(&root["output_bias"], "output_bias")?;
        let sha256 = format!("{:x}", Sha256::digest(contents.as_bytes()));
        Ok(Self {
            hidden_size,
            input_weights,
            hidden_bias,
            output_weights,
            output_bias,
            sha256,
        })
    }

    pub fn schema(&self) -> &str {
        MODEL_SCHEMA
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn hidden_size(&self) -> usize {
        self.hidden_size
    }

    /// Larger scores are preferred. A score is neither a probability nor a
    /// proof of reachability. Inference includes feature construction.
    pub fn score(&self, position: Position) -> f64 {
        self.score_features(&features(position))
    }

    fn score_features(&self, values: &[f64; FEATURE_COUNT]) -> f64 {
        let dot = |weights: &[f64]| {
            weights
                .iter()
                .zip(values)
                .map(|(weight, value)| weight * value)
                .sum::<f64>()
        };
        if self.hidden_size == 0 {
            self.output_bias + dot(&self.input_weights)
        } else {
            let mut value = self.output_bias;
            for hidden in 0..self.hidden_size {
                let offset = hidden * FEATURE_COUNT;
                let activation = (dot(&self.input_weights[offset..offset + FEATURE_COUNT])
                    + self.hidden_bias[hidden])
                    .max(0.0);
                value += self.output_weights[hidden] * activation;
            }
            value
        }
    }
}

/// Order-preserving mapping for finite signed scores; smaller keys are better.
pub(crate) fn score_priority(score: f64) -> io::Result<u128> {
    if !score.is_finite() {
        return Err(invalid("ranker produced a nonfinite score"));
    }
    let bits = score.to_bits();
    let ordered = if bits >> 63 == 1 {
        !bits
    } else {
        bits ^ (1_u64 << 63)
    };
    Ok(u128::from(u64::MAX - ordered))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::parse_position;

    #[test]
    fn feature_planes_include_the_exact_turn_and_have_documented_bounds() {
        let mut initial = Position::initial();
        let original = features(initial);
        initial.side_to_move = initial.side_to_move.opposite();
        let changed = features(initial);
        assert_eq!(&original[..128], &changed[..128]);
        assert_ne!(original[128], changed[128]);
        for text in [
            "OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO w",
            "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX b",
        ] {
            for value in features(parse_position(text).unwrap()) {
                assert!((0.0..=1.0).contains(&value));
            }
        }
    }

    #[test]
    fn finite_float_ranks_are_monotone_and_nonfinite_scores_are_rejected() {
        let scores = [-1e100, -100.0, -1.0, -0.0, 0.0, 1.0, 100.0, 1e100];
        for pair in scores.windows(2) {
            assert!(score_priority(pair[0]).unwrap() >= score_priority(pair[1]).unwrap());
        }
        for score in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(score_priority(score).is_err());
        }
    }

    #[test]
    fn a_small_network_matches_a_hand_computed_score() {
        let mut weights = vec![0.0; 2 * FEATURE_COUNT];
        weights[129] = 2.0;
        weights[FEATURE_COUNT + 128] = -3.0;
        let model = LearnedModel::from_json(
            &serde_json::json!({
                "schema": MODEL_SCHEMA,
                "feature_version": FEATURE_VERSION,
                "feature_count": FEATURE_COUNT,
                "hidden_size": 2,
                "input_weights": weights,
                "hidden_bias": [1.0, 2.0],
                "output_weights": [4.0, 5.0],
                "output_bias": -0.5,
            })
            .to_string(),
        )
        .unwrap();
        // Initial occupancy is 4/64; second activation is relu(-3 + 2).
        assert_eq!(model.score(Position::initial()), 4.0);
    }
}
