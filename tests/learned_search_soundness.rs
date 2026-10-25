//! Learned scores are adversarial inputs, not reachability evidence.
//!
//! The expected reachable set and every returned certificate are checked by
//! the independent absolute-color array oracle in tests/support. No expected
//! classification is copied from GBFS or the production bitboard successor.

#[allow(dead_code)]
mod support;

use std::collections::HashSet;

use othello_complexity_rs::othello::Position;
use othello_complexity_rs::record::{GoalMatch, PositionTransform};
use othello_complexity_rs::search::{
    beam::{retrospective_learned_beam_search, BeamConfig, BeamHeuristic},
    core::SearchOutcome,
    forward::ForwardTable,
    learned::{LearnedModel, FEATURE_COUNT, FEATURE_VERSION, MODEL_SCHEMA},
};
use serde_json::{json, Value};

use support::{enumerate, query_targets, ArrayPosition};

fn linear_json(weights: Vec<f64>, bias: f64) -> Value {
    json!({
        "schema": MODEL_SCHEMA,
        "feature_version": FEATURE_VERSION,
        "feature_count": FEATURE_COUNT,
        "hidden_size": 0,
        "input_weights": weights,
        "hidden_bias": [],
        "output_weights": [],
        "output_bias": bias
    })
}

fn from_value(value: Value) -> LearnedModel {
    LearnedModel::from_json(&value.to_string()).unwrap()
}

fn adversarial_models() -> Vec<LearnedModel> {
    let weights: Vec<_> = (0..FEATURE_COUNT)
        .map(|index| ((index * 37) % 101) as f64 - 50.0)
        .collect();
    let inverse: Vec<_> = weights.iter().map(|weight| -weight).collect();
    let hidden_weights: Vec<_> = weights.iter().chain(inverse.iter()).copied().collect();
    vec![
        from_value(linear_json(vec![0.0; FEATURE_COUNT], -1_000_000.0)),
        from_value(linear_json(weights, 1_000_000.0)),
        from_value(linear_json(inverse, -1_000_000.0)),
        from_value(json!({
            "schema": MODEL_SCHEMA,
            "feature_version": FEATURE_VERSION,
            "feature_count": FEATURE_COUNT,
            "hidden_size": 2,
            "input_weights": hidden_weights,
            "hidden_bias": [-17.0, 11.0],
            "output_weights": [-1_000_000.0, 1_000_000.0],
            "output_bias": -1_000_000.0
        })),
    ]
}

fn query_set(reachable: &HashSet<ArrayPosition>) -> Vec<ArrayPosition> {
    let mut targets = query_targets(reachable);
    let reversed_turns: Vec<_> = targets.iter().map(|goal| goal.opposite_turn()).collect();
    targets.extend(reversed_turns);
    targets.sort_by_key(|goal| goal.position());
    targets.dedup();
    targets
}

fn verify_sound_outcome(goal: ArrayPosition, reachable: bool, outcome: &SearchOutcome) {
    match outcome {
        SearchOutcome::Found { record, .. } => {
            assert!(
                reachable,
                "learned search claimed an oracle-unreachable goal"
            );
            assert_eq!(record.start, ArrayPosition::initial().position());
            assert_eq!(record.goal, goal.position());
            assert_eq!(record.goal_match, GoalMatch::Exact);
            assert_eq!(record.transform, PositionTransform::IDENTITY);
            let independent_trace = ArrayPosition::initial()
                .replay(&record.actions)
                .expect("learned search returned an illegal certificate");
            assert_eq!(independent_trace.last(), Some(&goal));
            let independent_positions: Vec<_> = independent_trace
                .into_iter()
                .map(ArrayPosition::position)
                .collect();
            assert_eq!(record.verify().unwrap(), independent_positions);
        }
        SearchOutcome::Unknown { .. } => {}
        SearchOutcome::NotFound { .. } => {
            panic!("learned search must never turn score/order/pruning into an NG decision");
        }
    }
}

#[test]
fn arbitrary_rankers_preserve_soundness_on_all_seven_stone_oracle_queries() {
    let reachable = enumerate(ArrayPosition::initial(), 7);
    let targets = query_set(&reachable);
    assert!(targets.iter().any(|goal| !reachable.contains(goal)));
    let table = ForwardTable::new(4, None).unwrap();
    let mut found = 0;
    let mut unknown = 0;
    let mut evaluated = 0;
    for model in adversarial_models() {
        for width in [1, usize::MAX] {
            for &goal in &targets {
                let expected = reachable.contains(&goal);
                let report = retrospective_learned_beam_search(
                    goal.position(),
                    &table,
                    &BeamConfig {
                        width,
                        max_nodes: 100_000,
                        heuristic: BeamHeuristic::Mobility,
                    },
                    &model,
                )
                .unwrap();
                verify_sound_outcome(goal, expected, &report.outcome);
                if width == usize::MAX && expected {
                    assert!(matches!(report.outcome, SearchOutcome::Found { .. }));
                }
                found += usize::from(matches!(report.outcome, SearchOutcome::Found { .. }));
                unknown += usize::from(matches!(report.outcome, SearchOutcome::Unknown { .. }));
                evaluated += report.stats.model_evaluations;
            }
        }
    }
    assert!(found > 0 && unknown > 0 && evaluated > 0);
}

#[test]
fn budgets_boundary_misses_and_impossible_roots_never_return_ng() {
    let table = ForwardTable::new(4, None).unwrap();
    let first = ArrayPosition::initial()
        .apply(othello_complexity_rs::record::PlyAction::Play(19))
        .unwrap();
    let at_boundary_but_wrong_turn = ArrayPosition::initial().opposite_turn();
    for model in adversarial_models() {
        for goal in [first, at_boundary_but_wrong_turn] {
            let report = retrospective_learned_beam_search(
                goal.position(),
                &table,
                &BeamConfig {
                    width: 1,
                    max_nodes: 0,
                    heuristic: BeamHeuristic::Branching,
                },
                &model,
            )
            .unwrap();
            assert!(matches!(
                report.outcome,
                SearchOutcome::Unknown { nodes: 0 }
            ));
        }
        let report = retrospective_learned_beam_search(
            Position::initial(),
            &table,
            &BeamConfig {
                width: 1,
                max_nodes: 0,
                heuristic: BeamHeuristic::Branching,
            },
            &model,
        )
        .unwrap();
        verify_sound_outcome(ArrayPosition::initial(), true, &report.outcome);
        assert!(matches!(
            report.outcome,
            SearchOutcome::Found { nodes: 0, .. }
        ));

        let unreachable = othello_complexity_rs::othello::Position {
            board: othello_complexity_rs::othello::Board {
                player: othello_complexity_rs::othello::CENTER_MASK | 1,
                opponent: 0,
            },
            side_to_move: othello_complexity_rs::othello::Color::Black,
        };
        let report = retrospective_learned_beam_search(
            unreachable,
            &table,
            &BeamConfig {
                width: usize::MAX,
                max_nodes: 100_000,
                heuristic: BeamHeuristic::Mobility,
            },
            &model,
        )
        .unwrap();
        assert!(matches!(report.outcome, SearchOutcome::Unknown { .. }));
    }
}

#[test]
fn invalid_models_are_rejected_before_they_can_guide_search() {
    let valid = linear_json(vec![0.0; FEATURE_COUNT], 0.0);
    let modifications = [
        ("schema", json!("different-model-v1")),
        ("feature_version", json!("different-board-features")),
        ("feature_count", json!(FEATURE_COUNT - 1)),
        ("hidden_size", json!(-1)),
        ("hidden_size", json!(129)),
        ("hidden_size", json!(0.5)),
        ("input_weights", json!([])),
        ("hidden_bias", json!([0.0])),
        ("output_weights", json!([0.0])),
        ("output_bias", Value::Null),
        ("output_bias", json!("NaN")),
        ("output_bias", json!(1_000_001.0)),
    ];
    for (field, replacement) in modifications {
        let mut invalid = valid.clone();
        invalid[field] = replacement;
        assert!(
            LearnedModel::from_json(&invalid.to_string()).is_err(),
            "accepted invalid model field {field}"
        );
    }
    let mut invalid = valid.clone();
    invalid["input_weights"][0] = json!(-1_000_001.0);
    assert!(LearnedModel::from_json(&invalid.to_string()).is_err());
    invalid["input_weights"][0] = json!("Infinity");
    assert!(LearnedModel::from_json(&invalid.to_string()).is_err());
    for invalid in [
        "null",
        "[]",
        "{}",
        "{\"output_bias\":1e999}",
        "{\"output_bias\":NaN}",
    ] {
        assert!(LearnedModel::from_json(invalid).is_err());
    }
}

#[test]
fn maximum_accepted_parameter_magnitudes_still_produce_finite_scores() {
    let models = [
        from_value(linear_json(vec![1_000_000.0; FEATURE_COUNT], -1_000_000.0)),
        from_value(json!({
            "schema": MODEL_SCHEMA,
            "feature_version": FEATURE_VERSION,
            "feature_count": FEATURE_COUNT,
            "hidden_size": 128,
            "input_weights": vec![1_000_000.0; FEATURE_COUNT * 128],
            "hidden_bias": vec![1_000_000.0; 128],
            "output_weights": vec![-1_000_000.0; 128],
            "output_bias": 1_000_000.0
        })),
    ];
    let mut positions: Vec<_> = enumerate(ArrayPosition::initial(), 7)
        .into_iter()
        .map(ArrayPosition::position)
        .collect();
    positions.push(
        othello_complexity_rs::io::parse_position(
            "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX b",
        )
        .unwrap(),
    );
    for model in models {
        for &position in &positions {
            assert!(model.score(position).is_finite());
        }
    }
}

#[test]
fn wrong_target_table_and_zero_width_are_errors_not_classifications() {
    let initial = ArrayPosition::initial();
    let goal = initial
        .apply(othello_complexity_rs::record::PlyAction::Play(19))
        .unwrap()
        .position();
    let other = initial
        .apply(othello_complexity_rs::record::PlyAction::Play(26))
        .unwrap()
        .position();
    let model = from_value(linear_json(vec![0.0; FEATURE_COUNT], 0.0));
    let targeted = ForwardTable::new(4, Some(goal)).unwrap();
    assert!(
        retrospective_learned_beam_search(other, &targeted, &BeamConfig::default(), &model)
            .is_err()
    );
    let table = ForwardTable::new(4, None).unwrap();
    assert!(retrospective_learned_beam_search(
        goal,
        &table,
        &BeamConfig {
            width: 0,
            ..BeamConfig::default()
        },
        &model
    )
    .is_err());
}
