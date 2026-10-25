//! Check exported NumPy rankers against Rust feature construction/inference.

use clap::Parser;
use othello_complexity_rs::io::parse_position;
use othello_complexity_rs::search::learned::{
    features, LearnedModel, FEATURE_COUNT, FEATURE_VERSION,
};
use serde_json::{json, Value};
use std::error::Error;
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    /// JSON parity cases produced by train_reverse_ranker.py.
    #[arg(long)]
    cases: PathBuf,
    /// Model filename stems must match the score keys, e.g. linear and mlp16.
    #[arg(long, required = true)]
    model: Vec<PathBuf>,
    #[arg(long)]
    output: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let root: Value = serde_json::from_slice(&fs::read(&args.cases)?)?;
    if root["feature_version"].as_str() != Some(FEATURE_VERSION) {
        return Err(io::Error::other("parity feature definition mismatch").into());
    }
    let cases = root["cases"]
        .as_array()
        .filter(|cases| !cases.is_empty())
        .ok_or_else(|| io::Error::other("parity cases must be a nonempty array"))?;
    let models: Vec<_> = args
        .model
        .iter()
        .map(|path| {
            let name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or_else(|| io::Error::other("model filename has no UTF-8 stem"))?;
            Ok((name.to_string(), LearnedModel::load(path)?))
        })
        .collect::<io::Result<_>>()?;
    let mut max_feature_error = 0.0_f64;
    let mut max_score_error = vec![0.0_f64; models.len()];
    for case in cases {
        let position = parse_position(
            case["position"]
                .as_str()
                .ok_or_else(|| io::Error::other("parity case has no position"))?,
        )
        .map_err(io::Error::other)?;
        let expected_features = case["features"]
            .as_array()
            .filter(|values| values.len() == FEATURE_COUNT)
            .ok_or_else(|| io::Error::other("parity feature count mismatch"))?;
        for (actual, expected) in features(position).into_iter().zip(expected_features) {
            let expected = expected
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| io::Error::other("nonfinite expected feature"))?;
            let error = (actual - expected).abs();
            max_feature_error = max_feature_error.max(error);
            if error > 1e-12 {
                return Err(io::Error::other("Rust/Python feature mismatch").into());
            }
        }
        for (index, (name, model)) in models.iter().enumerate() {
            let expected = case["scores"][name]
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| io::Error::other("missing finite Python score"))?;
            let actual = model.score(position);
            let error = (actual - expected).abs();
            max_score_error[index] = max_score_error[index].max(error);
            if !actual.is_finite() || error > 1e-9 + 1e-10 * expected.abs() {
                return Err(io::Error::other(format!(
                    "Rust/Python score mismatch for {name}: {actual} versus {expected}"
                ))
                .into());
            }
        }
    }
    let result = json!({
        "schema": "othello-ranker-parity-check-v1",
        "cases": cases.len(),
        "max_feature_absolute_error": max_feature_error,
        "models": models.iter().enumerate().map(|(index, (name, model))| json!({
            "name": name,
            "sha256": model.sha256(),
            "max_score_absolute_error": max_score_error[index],
        })).collect::<Vec<_>>(),
        "passed": true,
    });
    let rendered = format!("{}\n", serde_json::to_string_pretty(&result)?);
    if let Some(path) = args.output {
        fs::write(path, &rendered)?;
    }
    print!("{rendered}");
    Ok(())
}
