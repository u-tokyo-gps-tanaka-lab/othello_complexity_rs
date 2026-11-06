//! Batch reverse-query classification and result output.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering},
    mpsc,
};

use crate::io::{ensure_tri_outputs, format_position, TriCategory, TriOutcome, TriOutputs};
use crate::othello::Position;
use crate::query::{solve_query, ExactOutcome, GoalClassification};
use crate::record::GoalMatch;
use crate::search::{forward::ForwardTable, outcome::SearchOutcome};

fn classify_position(
    input_index: usize,
    position: Position,
    discs: u32,
    goal_match: GoalMatch,
    table_pool: Option<&rayon::ThreadPool>,
    mut solve: impl FnMut(usize, Position, &ForwardTable) -> io::Result<SearchOutcome> + Send,
) -> io::Result<(GoalClassification, usize)> {
    if goal_match == GoalMatch::EquivalenceClass {
        if input_index == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "input index must be at least 1",
            ));
        }
        let mut search = || {
            let table = ForwardTable::for_equivalence(discs, position)?;
            solve(input_index, position, &table)
        };
        let outcome = match table_pool {
            Some(pool) => pool.install(search),
            None => search(),
        }?;
        let nodes = outcome.nodes();
        let classification = match outcome {
            SearchOutcome::Found { mut record, .. } => {
                if record.start != Position::initial()
                    || record.goal != position
                    || record.goal_match != GoalMatch::EquivalenceClass
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "equivalence search returned a record for another query",
                    ));
                }
                record.input_index = input_index;
                record
                    .verify()
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                GoalClassification::Reachable { record }
            }
            SearchOutcome::NotFound { .. } => GoalClassification::Unreachable {
                input_index,
                goal: position,
                goal_match,
            },
            SearchOutcome::Unknown { .. } => GoalClassification::Unknown {
                input_index,
                goal: position,
                goal_match,
            },
        };
        return Ok((classification, nodes));
    }

    let mut nodes = 0_usize;
    let outcome = solve_query(
        Position::initial(),
        position,
        input_index,
        goal_match,
        |candidate, _transform| {
            let mut search = || {
                let table = ForwardTable::new(discs, Some(candidate))?;
                solve(input_index, candidate, &table)
            };
            let outcome = match table_pool {
                Some(pool) => pool.install(search),
                None => search(),
            }
            .map_err(|error| error.to_string())?;
            nodes = nodes
                .checked_add(outcome.nodes())
                .ok_or("reverse query node count overflow")?;
            Ok(Some(match outcome {
                SearchOutcome::Found { record, .. } => ExactOutcome::Reachable(record.actions),
                SearchOutcome::NotFound { .. } => ExactOutcome::Unreachable,
                SearchOutcome::Unknown { .. } => ExactOutcome::Unknown,
            }))
        },
    )
    .map_err(io::Error::other)?
    .ok_or_else(|| io::Error::other("reverse query produced no classification"))?;
    Ok((outcome, nodes))
}

fn write_query_result(
    outputs: &mut TriOutputs,
    records: &mut BufWriter<File>,
    result: &(GoalClassification, usize),
) -> io::Result<()> {
    let (outcome, nodes) = result;
    if let Some(record) = outcome.record() {
        writeln!(records, "{}", record.to_csv())?;
        records.flush()?;
    }
    let line = format_position(outcome.goal());
    outputs.write_result(outcome, &line)?;
    outputs.flush()?;
    let category = match outcome.category() {
        TriCategory::Ok => "OK",
        TriCategory::Ng => "NG",
        TriCategory::Unknown => "UNKNOWN",
    };
    println!("input {}: {category} nodes={nodes}", outcome.input_index());
    Ok(())
}

pub(super) fn run_queries(
    positions: &[Position],
    out_dir: &Path,
    discs: u32,
    goal_match: GoalMatch,
    mut solve: impl FnMut(usize, Position, &ForwardTable) -> io::Result<SearchOutcome> + Send,
) -> io::Result<Vec<(GoalClassification, usize)>> {
    let prefix = format!("reverse_{goal_match}");
    let mut outputs = ensure_tri_outputs(out_dir, &prefix)?;
    let mut records = BufWriter::new(File::create(out_dir.join(format!("{prefix}_records.csv")))?);
    let mut results = Vec::with_capacity(positions.len());
    for (index, position) in positions.iter().copied().enumerate() {
        let result = classify_position(index + 1, position, discs, goal_match, None, &mut solve)?;
        write_query_result(&mut outputs, &mut records, &result)?;
        results.push(result);
    }
    records.flush()?;
    outputs.flush()?;
    Ok(results)
}

pub(super) fn run_parallel_queries<F>(
    positions: &[Position],
    out_dir: &Path,
    discs: u32,
    goal_match: GoalMatch,
    parallel_goals: usize,
    threads: usize,
    solve: F,
) -> io::Result<Vec<(GoalClassification, usize)>>
where
    F: Fn(usize, Position, &ForwardTable) -> io::Result<SearchOutcome> + Sync,
{
    let prefix = format!("reverse_{goal_match}");
    let mut outputs = ensure_tri_outputs(out_dir, &prefix)?;
    let mut records = BufWriter::new(File::create(out_dir.join(format!("{prefix}_records.csv")))?);
    if positions.is_empty() {
        records.flush()?;
        outputs.flush()?;
        return Ok(Vec::new());
    }

    let cancelled = AtomicBool::new(false);
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();
    let workers = parallel_goals.min(positions.len());
    let mut results: Vec<Option<(GoalClassification, usize)>> = std::iter::repeat_with(|| None)
        .take(positions.len())
        .collect();
    let mut first_error = None;

    std::thread::scope(|scope| {
        let worker_cancelled = &cancelled;
        let worker_next = &next;
        let worker_solve = &solve;
        for _ in 0..workers {
            let tx = tx.clone();
            scope.spawn(move || {
                let pool = match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
                    Ok(pool) => pool,
                    Err(error) => {
                        worker_cancelled.store(true, AtomicOrdering::Release);
                        let _ = tx.send(Err(io::Error::other(format!(
                            "failed to build rayon pool for a reverse-search worker: {error}"
                        ))));
                        return;
                    }
                };
                loop {
                    if worker_cancelled.load(AtomicOrdering::Acquire) {
                        return;
                    }
                    let index = worker_next.fetch_add(1, AtomicOrdering::Relaxed);
                    let Some(&position) = positions.get(index) else {
                        return;
                    };
                    if worker_cancelled.load(AtomicOrdering::Acquire) {
                        return;
                    }
                    let result = classify_position(
                        index + 1,
                        position,
                        discs,
                        goal_match,
                        Some(&pool),
                        worker_solve,
                    )
                    .map(|result| (index, result));
                    if result.is_err() {
                        worker_cancelled.store(true, AtomicOrdering::Release);
                    }
                    if tx.send(result).is_err() {
                        worker_cancelled.store(true, AtomicOrdering::Release);
                        return;
                    }
                }
            });
        }
        drop(tx);

        for received in rx {
            match received {
                Ok((index, result)) if first_error.is_none() => {
                    if let Err(error) = write_query_result(&mut outputs, &mut records, &result) {
                        cancelled.store(true, AtomicOrdering::Release);
                        first_error = Some(error);
                    } else {
                        results[index] = Some(result);
                    }
                }
                Ok(_) => {}
                Err(error) if first_error.is_none() => {
                    cancelled.store(true, AtomicOrdering::Release);
                    first_error = Some(error);
                }
                Err(_) => {}
            }
        }
    });

    if let Some(error) = first_error {
        return Err(error);
    }
    records.flush()?;
    outputs.flush()?;
    results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.ok_or_else(|| {
                io::Error::other(format!(
                    "parallel reverse search produced no result for input {}",
                    index + 1
                ))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::record::ReachabilityRecord;
    use crate::search::{
        engines::parallel_gbfs::parallel_retrospective_greedy_best_first_search,
        moves::successors,
        run_strategy,
        test_support::{
            bfs_strategy, gbfs_strategy, reachable_position_with_discs, TemporaryDirectory,
        },
        Strategy,
    };

    fn query_positions_with_duplicate() -> [Position; 4] {
        let after_move = successors(Position::initial())[0].0;
        let wrong_turn = Position {
            side_to_move: after_move.side_to_move.opposite(),
            ..after_move
        };
        [Position::initial(), after_move, wrong_turn, after_move]
    }

    fn output_lines(out_dir: &Path, suffix: &str) -> Vec<String> {
        fs::read_to_string(out_dir.join(format!("reverse_exact_{suffix}.txt")))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn output_records(out_dir: &Path) -> Vec<ReachabilityRecord> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_path(out_dir.join("reverse_exact_records.csv"))
            .unwrap();
        reader
            .records()
            .map(|fields| ReachabilityRecord::try_from(&fields.unwrap()).unwrap())
            .collect()
    }

    #[test]
    fn bfs_and_gbfs_goal_counts_preserve_serial_order_and_match_outputs() {
        let directory = TemporaryDirectory::new("parallel-goal-counts");
        let positions = query_positions_with_duplicate();
        for (name, serial_strategy, parallel_strategy) in [
            ("gbfs", gbfs_strategy(1, 1), gbfs_strategy(4, 1)),
            ("bfs", bfs_strategy(1, 1), bfs_strategy(4, 1)),
        ] {
            let serial_dir = directory.join(format!("{name}-serial"));
            let parallel_dir = directory.join(format!("{name}-parallel"));
            let serial =
                run_strategy(&serial_strategy, &positions, &serial_dir, GoalMatch::Exact).unwrap();
            let parallel = run_strategy(
                &parallel_strategy,
                &positions,
                &parallel_dir,
                GoalMatch::Exact,
            )
            .unwrap();

            assert_eq!(serial.len(), positions.len(), "{name}");
            for (index, ((classification, _), position)) in serial.iter().zip(positions).enumerate()
            {
                assert_eq!(classification.input_index(), index + 1, "{name}");
                assert_eq!(classification.goal(), position, "{name}");
            }
            let after_move = format_position(positions[1]);
            assert_eq!(
                output_lines(&serial_dir, "OK"),
                [
                    format_position(positions[0]),
                    after_move.clone(),
                    after_move
                ],
                "{name}"
            );
            assert_eq!(
                output_lines(&serial_dir, "NG"),
                [format_position(positions[2])],
                "{name}"
            );
            assert!(output_lines(&serial_dir, "UNKNOWN").is_empty(), "{name}");

            assert_eq!(parallel, serial, "{name}");
            for suffix in ["OK", "NG", "UNKNOWN"] {
                let mut serial_lines = output_lines(&serial_dir, suffix);
                let mut parallel_lines = output_lines(&parallel_dir, suffix);
                serial_lines.sort_unstable();
                parallel_lines.sort_unstable();
                assert_eq!(parallel_lines, serial_lines, "{name}-{suffix}");
            }
            let mut serial_records = output_records(&serial_dir);
            let mut parallel_records = output_records(&parallel_dir);
            serial_records.sort_by_key(|record| record.input_index);
            parallel_records.sort_by_key(|record| record.input_index);
            assert_eq!(parallel_records, serial_records, "{name}");
            assert_eq!(
                parallel_records
                    .iter()
                    .map(|record| record.input_index)
                    .collect::<Vec<_>>(),
                [1, 2, 4],
                "{name}"
            );
            for record in parallel_records {
                record.verify().unwrap();
            }
        }
    }

    #[test]
    fn parallel_query_error_cancels_unstarted_positions() {
        let directory = TemporaryDirectory::new("parallel-query-error");
        let positions = vec![Position::initial(); 32];
        let started = AtomicUsize::new(0);

        let error = run_parallel_queries(
            &positions,
            &directory.0,
            4,
            GoalMatch::Exact,
            4,
            1,
            |input_index, _, _| {
                started.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::other(format!("failure at input {input_index}")))
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("failure at input"));
        assert!((1..=4).contains(&started.load(Ordering::SeqCst)));
        for suffix in ["OK", "NG", "UNKNOWN"] {
            assert!(output_lines(&directory.0, suffix).is_empty());
        }
        assert!(output_records(&directory.0).is_empty());
    }

    #[test]
    fn empty_parallel_query_batch_creates_empty_outputs() {
        let directory = TemporaryDirectory::new("empty-parallel-query-batch");
        let results = run_parallel_queries(
            &[],
            &directory.0,
            4,
            GoalMatch::Exact,
            4,
            1,
            |_, _, _| -> io::Result<SearchOutcome> {
                panic!("an empty batch must not invoke solve")
            },
        )
        .unwrap();

        assert!(results.is_empty());
        for suffix in ["OK", "NG", "UNKNOWN"] {
            assert!(output_lines(&directory.0, suffix).is_empty());
        }
        assert!(output_records(&directory.0).is_empty());
    }

    #[test]
    fn candidate_table_rejects_a_different_goal() {
        let goals = successors(Position::initial());
        let goal = goals[0].0;
        let different_goal = goals[1].0;

        let (classification, nodes) =
            classify_position(1, goal, 4, GoalMatch::Exact, None, |_, candidate, table| {
                table.validate_goal(candidate)?;
                let error = table.validate_goal(different_goal).unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
                Ok(SearchOutcome::Unknown { nodes: 7 })
            })
            .unwrap();

        assert_eq!(classification.goal(), goal);
        assert_eq!(classification.category(), TriCategory::Unknown);
        assert_eq!(nodes, 7);
    }

    #[test]
    fn equivalence_classification_searches_the_input_once() {
        let goal = successors(Position::initial())[0].0;
        let calls = AtomicUsize::new(0);

        let (classification, nodes) = classify_position(
            3,
            goal,
            4,
            GoalMatch::EquivalenceClass,
            None,
            |input_index, candidate, table| {
                calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(input_index, 3);
                assert_eq!(candidate, goal);
                assert_eq!(table.goal_match(), GoalMatch::EquivalenceClass);
                table.validate_goal(goal)?;
                Ok(SearchOutcome::NotFound { nodes: 11 })
            },
        )
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(nodes, 11);
        assert_eq!(classification.input_index(), 3);
        assert_eq!(classification.goal(), goal);
        assert_eq!(classification.goal_match(), GoalMatch::EquivalenceClass);
        assert_eq!(classification.category(), TriCategory::Ng);
    }

    #[test]
    fn table_pool_covers_gbfs_with_scoped_workers() {
        let goal = reachable_position_with_discs(7);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        let pool_threads = AtomicUsize::new(0);

        let (classification, nodes) = classify_position(
            1,
            goal,
            4,
            GoalMatch::Exact,
            Some(&pool),
            |_, candidate, table| {
                pool_threads.store(rayon::current_num_threads(), Ordering::SeqCst);
                parallel_retrospective_greedy_best_first_search(
                    candidate,
                    table,
                    usize::MAX,
                    false,
                    2,
                )
            },
        )
        .unwrap();

        assert_eq!(pool_threads.load(Ordering::SeqCst), 2);
        assert!(nodes > 0);
        classification.record().unwrap().verify().unwrap();
    }

    #[test]
    fn mixed_disc_positions_preserve_input_order_and_duplicates() {
        let directory = TemporaryDirectory::new("mixed-stone-counts");
        let initial = Position::initial();
        let five = reachable_position_with_discs(5);
        let positions = [initial, five, reachable_position_with_discs(10), initial];

        let results = run_strategy(
            &Strategy::Dfs {
                discs: 10,
                max_nodes: 1,
            },
            &positions,
            &directory.0,
            GoalMatch::Exact,
        )
        .unwrap();

        assert_eq!(results.len(), positions.len());
        for (index, ((classification, _), expected)) in results.iter().zip(positions).enumerate() {
            assert_eq!(classification.input_index(), index + 1);
            assert_eq!(classification.goal(), expected);
            classification
                .record()
                .expect("each fixed opening position is reachable")
                .verify()
                .unwrap();
        }
        assert_eq!(
            output_lines(&directory.0, "OK"),
            positions.map(format_position)
        );
        assert!(output_lines(&directory.0, "NG").is_empty());
        assert!(output_lines(&directory.0, "UNKNOWN").is_empty());
        assert_eq!(results[0].0.goal(), results[3].0.goal());
        assert_ne!(results[0].0.input_index(), results[3].0.input_index());
    }
}
