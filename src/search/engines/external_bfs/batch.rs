//! Checkpointed batches of external breadth-first searches.

use std::fs;
use std::io;
use std::path::Path;

use super::{
    io::{read_json, save_json},
    search::{parallel_retrospective_bfs, sequential_retrospective_bfs, validate_fresh_directory},
    ExternalBfs,
};
use crate::io::format_position;
use crate::othello::Position;
use crate::query::GoalClassification;
use crate::record::GoalMatch;
use crate::search::batch::run_queries;

pub(in crate::search) fn run_external(
    cfg: &ExternalBfs,
    positions: &[Position],
    out_dir: &Path,
    goal_match: GoalMatch,
    parallel: bool,
) -> io::Result<Vec<(GoalClassification, usize)>> {
    use serde_json::{json, Value};
    let candidate_count = match goal_match {
        GoalMatch::Exact => positions.iter().try_fold(0_usize, |total, &goal| {
            total
                .checked_add(crate::query::candidates(Position::initial(), goal, goal_match).len())
                .ok_or_else(|| io::Error::other("candidate count overflow"))
        })?,
        GoalMatch::EquivalenceClass => positions.len(),
    };
    let identity = json!({"mode": goal_match.as_str(),
        "positions": positions.iter().copied().map(format_position).collect::<Vec<_>>(),
        "discs": cfg.discs, "jobs": cfg.jobs,
        "block_size": cfg.block_size, "parallel": parallel});
    fs::create_dir_all(&cfg.tmp_dir)?;
    let manifest_path = cfg.tmp_dir.join("batch.json");
    let mut manifest = if cfg.resume {
        let saved = read_json(&manifest_path)?;
        if saved.get("identity") != Some(&identity) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "saved batch input or settings mismatch",
            ));
        }
        saved
    } else {
        if fs::read_dir(&cfg.tmp_dir)?.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "temporary directory is not empty",
            ));
        }
        json!({"identity": identity, "completed": 0, "active": null})
    };
    let completed = manifest
        .get("completed")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n <= candidate_count)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid batch progress"))?;
    let active = match manifest.get("active") {
        Some(Value::Null) => None,
        Some(number) => Some(
            number
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&n| n == completed + 1 && n <= candidate_count)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid active query")
                })?,
        ),
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "missing active query state",
            ))
        }
    };
    save_json(&manifest_path, &manifest)?;
    let mut candidate_index = 0;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(cfg.jobs)
        .build()
        .map_err(io::Error::other)?;
    pool.install(|| {
        run_queries(
            positions,
            out_dir,
            cfg.discs,
            goal_match,
            |_, goal, table| {
                candidate_index += 1;
                let index = candidate_index;
                let resume = cfg.resume && (index <= completed || active == Some(index));
                let query_cfg = ExternalBfs {
                    tmp_dir: cfg.tmp_dir.join(format!("candidate_{index}")),
                    resume,
                    ..cfg.clone()
                };
                if resume && index <= completed {
                    let saved_query = read_json(&query_cfg.tmp_dir.join("checkpoint.json"))?;
                    if !matches!(
                        saved_query.get("state").and_then(Value::as_str),
                        Some("found" | "not-found")
                    ) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "completed batch query has no completed search checkpoint",
                        ));
                    }
                }
                if index > completed {
                    if !resume {
                        validate_fresh_directory(&query_cfg.tmp_dir)?;
                    }
                    manifest["active"] = json!(index);
                    save_json(&manifest_path, &manifest)?;
                }
                let outcome = if parallel {
                    parallel_retrospective_bfs(&query_cfg, goal, table)?
                } else {
                    sequential_retrospective_bfs(&query_cfg, goal, table)?
                };
                if index > completed {
                    manifest["completed"] = json!(index);
                    manifest["active"] = Value::Null;
                    save_json(&manifest_path, &manifest)?;
                }
                Ok(outcome)
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    use crate::search::{
        moves::successors,
        run_strategy,
        test_support::{external_config, reachable_position_with_discs, TemporaryDirectory},
        Strategy,
    };

    #[test]
    fn external_equivalence_uses_one_checkpoint_directory_per_input() {
        let directory = TemporaryDirectory::new("external-equivalence-inputs");
        let tmp_dir = directory.join("tmp");
        let positions = [Position::initial(), successors(Position::initial())[0].0];
        let results = run_strategy(
            &Strategy::BfsExternal(external_config(4, 8, tmp_dir.clone())),
            &positions,
            &directory.join("out"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        assert_eq!(results.len(), positions.len());
        for (index, ((classification, _), goal)) in results.iter().zip(positions).enumerate() {
            let record = classification.record().unwrap();
            assert_eq!(record.input_index, index + 1);
            assert_eq!(record.goal, goal);
            assert_eq!(record.goal_match, GoalMatch::EquivalenceClass);
            record.verify().unwrap();
        }
        assert!(tmp_dir.join("candidate_1/checkpoint.json").is_file());
        assert!(tmp_dir.join("candidate_2/checkpoint.json").is_file());
        assert!(!tmp_dir.join("candidate_3").exists());
        let checkpoint: serde_json::Value = serde_json::from_reader(
            File::open(tmp_dir.join("candidate_1/checkpoint.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            checkpoint["identity"]["goal_match"],
            GoalMatch::EquivalenceClass.as_str()
        );
        let manifest: serde_json::Value =
            serde_json::from_reader(File::open(tmp_dir.join("batch.json")).unwrap()).unwrap();
        assert_eq!(manifest["completed"], positions.len());
    }

    #[test]
    fn completed_external_equivalence_search_resumes_with_the_same_record() {
        let directory = TemporaryDirectory::new("external-equivalence-completed-resume");
        let tmp_dir = directory.join("tmp");
        let goal = reachable_position_with_discs(7);
        let config = external_config(4, 4, tmp_dir);
        let first = run_strategy(
            &Strategy::BfsExternal(config.clone()),
            &[goal],
            &directory.join("first"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        let resumed = run_strategy(
            &Strategy::BfsExternal(ExternalBfs {
                resume: true,
                ..config
            }),
            &[goal],
            &directory.join("resumed"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        assert_eq!(resumed, first);
        resumed[0].0.record().unwrap().verify().unwrap();
    }

    #[test]
    fn active_external_equivalence_search_resumes_with_or_without_a_checkpoint() {
        for (name, has_checkpoint) in [
            ("without-query-checkpoint", false),
            ("with-query-checkpoint", true),
        ] {
            let directory = TemporaryDirectory::new(name);
            let tmp_dir = directory.join("tmp");
            let candidate_dir = tmp_dir.join("candidate_1");
            let goal = reachable_position_with_discs(7);
            let config = external_config(4, 4, tmp_dir.clone());

            let first = if has_checkpoint {
                let first = run_strategy(
                    &Strategy::BfsExternal(config.clone()),
                    &[goal],
                    &directory.join("first"),
                    GoalMatch::EquivalenceClass,
                )
                .unwrap();
                let checkpoint_path = candidate_dir.join("checkpoint.json");
                let mut checkpoint: serde_json::Value =
                    serde_json::from_reader(File::open(&checkpoint_path).unwrap()).unwrap();
                assert_eq!(checkpoint["state"], "found", "{name}");
                checkpoint["state"] = serde_json::json!("searching");
                checkpoint["record"] = serde_json::Value::Null;
                save_json(&checkpoint_path, &checkpoint).unwrap();

                let manifest_path = tmp_dir.join("batch.json");
                let mut manifest: serde_json::Value =
                    serde_json::from_reader(File::open(&manifest_path).unwrap()).unwrap();
                manifest["completed"] = serde_json::json!(0);
                manifest["active"] = serde_json::json!(1);
                save_json(&manifest_path, &manifest).unwrap();
                Some(first)
            } else {
                fs::create_dir_all(&candidate_dir).unwrap();
                fs::write(candidate_dir.join("layer_0.bin"), b"incomplete layer").unwrap();
                fs::write(candidate_dir.join("layer_0.pending"), b"incomplete layer").unwrap();
                fs::write(
                    candidate_dir.join("checkpoint.pending"),
                    b"incomplete checkpoint",
                )
                .unwrap();
                let identity = serde_json::json!({
                    "mode": GoalMatch::EquivalenceClass.as_str(),
                    "positions": [format_position(goal)],
                    "discs": config.discs,
                    "jobs": config.jobs,
                    "block_size": config.block_size,
                    "parallel": false,
                });
                save_json(
                    &tmp_dir.join("batch.json"),
                    &serde_json::json!({"identity": identity, "completed": 0, "active": 1}),
                )
                .unwrap();
                None
            };

            let resumed = run_strategy(
                &Strategy::BfsExternal(ExternalBfs {
                    resume: true,
                    ..config
                }),
                &[goal],
                &directory.join("resumed"),
                GoalMatch::EquivalenceClass,
            )
            .unwrap();

            assert_eq!(resumed.len(), 1, "{name}");
            if let Some(first) = first {
                assert_eq!(resumed[0].0, first[0].0, "{name}");
            }
            resumed[0].0.record().unwrap().verify().unwrap();
            assert!(candidate_dir.join("checkpoint.json").is_file(), "{name}");
            if !has_checkpoint {
                assert!(!candidate_dir.join("layer_0.pending").exists(), "{name}");
                assert!(!candidate_dir.join("checkpoint.pending").exists(), "{name}");
            }
            let manifest: serde_json::Value =
                serde_json::from_reader(File::open(tmp_dir.join("batch.json")).unwrap()).unwrap();
            assert_eq!(manifest["completed"], 1, "{name}");
            assert!(manifest["active"].is_null(), "{name}");
        }
    }

    #[test]
    fn unstarted_external_candidate_preserves_user_files_on_repeated_resume() {
        let directory = TemporaryDirectory::new("external-unstarted-user-file");
        let tmp_dir = directory.join("tmp");
        let positions = [Position::initial(), reachable_position_with_discs(7)];
        let config = external_config(4, 4, tmp_dir.clone());
        run_strategy(
            &Strategy::BfsExternal(config.clone()),
            &positions,
            &directory.join("initial"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap();

        let candidate_dir = tmp_dir.join("candidate_2");
        fs::remove_dir_all(&candidate_dir).unwrap();
        fs::create_dir(&candidate_dir).unwrap();
        let user_file = candidate_dir.join("notes.txt");
        fs::write(&user_file, b"user data").unwrap();
        let manifest_path = tmp_dir.join("batch.json");
        let mut manifest: serde_json::Value =
            serde_json::from_reader(File::open(&manifest_path).unwrap()).unwrap();
        manifest["completed"] = serde_json::json!(1);
        manifest["active"] = serde_json::Value::Null;
        save_json(&manifest_path, &manifest).unwrap();

        let resume = Strategy::BfsExternal(ExternalBfs {
            resume: true,
            ..config
        });
        for attempt in 1..=2 {
            let error = run_strategy(
                &resume,
                &positions,
                &directory.join(format!("resume-{attempt}")),
                GoalMatch::EquivalenceClass,
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(&user_file).unwrap(), b"user data");
        }
        let manifest: serde_json::Value =
            serde_json::from_reader(File::open(manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["completed"], 1);
        assert!(manifest["active"].is_null());
    }

    #[test]
    fn external_resume_rejects_a_different_goal_match_identity() {
        let directory = TemporaryDirectory::new("external-goal-match-resume");
        let tmp_dir = directory.join("tmp");
        let goal = reachable_position_with_discs(7);
        let config = external_config(4, 4, tmp_dir.clone());
        run_strategy(
            &Strategy::BfsExternal(config.clone()),
            &[goal],
            &directory.join("exact"),
            GoalMatch::Exact,
        )
        .unwrap();
        let resume = Strategy::BfsExternal(ExternalBfs {
            resume: true,
            ..config
        });

        let error = run_strategy(
            &resume,
            &[goal],
            &directory.join("equivalence"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("saved batch input or settings mismatch"));

        let manifest_path = tmp_dir.join("batch.json");
        let mut manifest: serde_json::Value =
            serde_json::from_reader(File::open(&manifest_path).unwrap()).unwrap();
        manifest["identity"]["mode"] = serde_json::json!("equivalence");
        save_json(&manifest_path, &manifest).unwrap();
        let error = run_strategy(
            &resume,
            &[goal],
            &directory.join("equivalence-inner"),
            GoalMatch::EquivalenceClass,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error
            .to_string()
            .contains("checkpoint query or search settings mismatch"));
    }
}
