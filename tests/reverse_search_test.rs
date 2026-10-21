//! Absolute-position regressions against the independent array-rules oracle.
//! The seven existing search strategies keep their original traversal and limits.

#[allow(dead_code)]
mod support;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use othello_complexity_rs::io::format_position;
use othello_complexity_rs::othello::{BoardSymmetry, Color, Position};
use othello_complexity_rs::record::PlyAction;
use othello_complexity_rs::search::{
    core::{prev_states, prev_states_with_buffer, SearchResult},
    dfs::retrospective_search,
    external_bfs::{
        expand_layer_blocked, merge_sorted_bins, parallel_retrospective_bfs,
        parallel_retrospective_bfs_resume, read_target_board, sequential_retrospective_bfs,
        unblocked_retrospective_bfs, Cfg,
    },
    forward::{fwd_search, make_fwd_table},
    inmemory_bfs::parallel_inmemory_retrospective_bfs,
    move_ordering::retrospective_search_move_ordering,
    parallel_dfs::retrospective_search_parallel,
    parallel_gbfs::parallel_retrospective_greedy_best_first_search,
    transposition::{Btable, LeafCache},
};
use support::ArrayPosition;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "reverse-position-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn external_config(directory: &std::path::Path, discs: i32) -> Cfg {
    Cfg {
        input: directory.join("input.txt"),
        out_dir: directory.to_path_buf(),
        jobs: 2,
        verbose: 0,
        block_size: 1,
        discs: discs as usize,
        tmp_dir: directory.to_path_buf(),
        resume: false,
    }
}

fn assert_all_searches(target: Position, discs: i32, leaves: &[Position], expected: SearchResult) {
    let mut sorted = leaves.to_vec();
    sorted.sort_unstable();
    let leaf_set = sorted.iter().copied().collect::<HashSet<_>>();
    let mut table = Btable::new(4096, 32);
    let mut buffers = Vec::new();
    let mut nodes = 0;
    let actual = retrospective_search(
        &target,
        false,
        discs,
        &leaf_set,
        &mut table,
        &mut buffers,
        &mut nodes,
        100_000,
    );
    assert_eq!(actual, expected, "DFS: {}", format_position(target));

    table.clear();
    nodes = 0;
    let actual = retrospective_search_move_ordering(
        &target,
        false,
        discs,
        &leaf_set,
        &mut table,
        &mut buffers,
        &mut nodes,
        100_000,
    );
    assert_eq!(actual, expected, "ordered DFS: {}", format_position(target));
    assert_eq!(
        retrospective_search_parallel(&target, false, discs, &leaf_set, 100_000, 4096),
        expected,
        "parallel DFS: {}",
        format_position(target),
    );
    assert_eq!(
        parallel_retrospective_greedy_best_first_search(&target, discs, &sorted, 100_000, false, 1),
        expected,
        "GBFS: {}",
        format_position(target),
    );
    assert_eq!(
        parallel_inmemory_retrospective_bfs(&target, discs, &sorted, 100_000, false, false),
        expected,
        "in-memory BFS: {}",
        format_position(target),
    );

    let sequential = TestDirectory::new();
    assert_eq!(
        unblocked_retrospective_bfs(
            &external_config(sequential.path(), discs),
            &target,
            discs,
            &sorted
        )
        .unwrap(),
        expected,
        "external BFS: {}",
        format_position(target),
    );
    let parallel = TestDirectory::new();
    assert_eq!(
        parallel_retrospective_bfs(
            &external_config(parallel.path(), discs),
            &target,
            discs,
            &sorted
        )
        .unwrap(),
        expected,
        "parallel external BFS: {}",
        format_position(target),
    );
}

#[test]
fn every_search_distinguishes_absolute_turn_and_board_orientation() {
    let initial = ArrayPosition::initial();
    let first = initial.apply(PlyAction::Play(19)).unwrap();
    let second = first.apply(PlyAction::Play(18)).unwrap();
    let reachable = support::enumerate(initial, 6);
    let targets = [
        initial,
        initial.color_exchanged(),
        initial.transformed(BoardSymmetry::Rotate90),
        first,
        first.color_exchanged(),
        first.transformed(BoardSymmetry::Rotate90),
        second,
        second.color_exchanged(),
    ];
    for target in targets {
        let expected = if reachable.contains(&target) {
            SearchResult::Found
        } else {
            SearchResult::NotFound
        };
        assert_all_searches(target.position(), 4, &[initial.position()], expected);
    }
}

#[test]
fn goals_below_the_requested_forward_boundary_use_exact_positions() {
    let initial = ArrayPosition::initial();
    let first = initial.apply(PlyAction::Play(19)).unwrap();
    let cache = LeafCache::new(6);
    assert!(cache.leaf().contains(&initial.position()));
    assert!(cache.leaf().contains(&first.position()));
    assert!(!cache.leaf().contains(&initial.color_exchanged().position()));
    assert!(!cache.leaf().contains(&first.color_exchanged().position()));

    for target in [initial, first] {
        let leaf = make_fwd_table(&target.position(), 6);
        assert!(leaf.contains(&target.position()));
        assert_all_searches(target.position(), 6, &leaf, SearchResult::Found);
        let wrong_turn = target.color_exchanged().position();
        let wrong_leaf = make_fwd_table(&wrong_turn, 6);
        assert_all_searches(wrong_turn, 6, &wrong_leaf, SearchResult::NotFound);
    }
}

fn early_pass_path() -> Vec<ArrayPosition> {
    let placements = [19, 18, 17, 9, 37, 16, 0, 2];
    ArrayPosition::initial()
        .replay(&placements.map(PlyAction::Play))
        .unwrap()
}

#[test]
fn pass_at_the_query_and_at_the_meeting_boundary_preserves_absolute_turn() {
    let path = early_pass_path();
    let before = *path.last().unwrap();
    assert!(before.moves().is_empty());
    let after = before.apply(PlyAction::Pass).unwrap();
    assert!(!after.moves().is_empty());
    let previous = path[path.len() - 2];
    for target in [before, after] {
        assert_all_searches(
            target.position(),
            11,
            &[previous.position()],
            SearchResult::Found,
        );
    }

    let forward = make_fwd_table(&after.position(), 12);
    assert!(forward.contains(&before.position()));
    assert!(forward.contains(&after.position()));
    for target in [before, after] {
        assert_all_searches(target.position(), 12, &forward, SearchResult::Found);
    }
    let next = after.apply(PlyAction::Play(after.moves()[0])).unwrap();
    assert_all_searches(next.position(), 12, &forward, SearchResult::Found);
}

#[test]
fn forward_boundary_keeps_prepass_and_terminal_states_without_terminal_passes() {
    let path = early_pass_path();
    let before = *path.last().unwrap();
    let after = before.apply(PlyAction::Pass).unwrap();
    let mut searched = HashSet::new();
    let mut leaf = HashSet::new();
    fwd_search(&before.position(), &mut searched, &mut leaf, 12);
    assert_eq!(leaf, HashSet::from([before.position(), after.position()]));

    let terminal = *support::middle_pass_start()
        .replay(&[PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)])
        .unwrap()
        .last()
        .unwrap();
    assert!(terminal.moves().is_empty());
    assert!(terminal.apply(PlyAction::Pass).is_err());
    leaf.clear();
    fwd_search(&terminal.position(), &mut searched, &mut leaf, 64);
    assert_eq!(leaf, HashSet::from([terminal.position()]));
}

#[test]
fn buffered_predecessors_replay_with_the_oracle_including_pass_before_placement() {
    let mut fixtures = support::enumerate(ArrayPosition::initial(), 6)
        .into_iter()
        .collect::<Vec<_>>();
    let before = *early_pass_path().last().unwrap();
    let after = before.apply(PlyAction::Pass).unwrap();
    fixtures.push(after.apply(PlyAction::Play(after.moves()[0])).unwrap());
    let mut buffer = [0; 10_000];
    let mut output = Vec::new();
    let mut saw_pass_then_placement = false;
    for target in fixtures {
        let target_position = target.position();
        prev_states_with_buffer(target_position, &mut buffer, &mut output);
        assert_eq!(output, prev_states(target_position));
        for &previous in &output {
            assert_eq!(
                previous.board.popcount() + 1,
                target_position.board.popcount()
            );
            let previous_oracle = ArrayPosition::from_relative(
                &format_position(previous)[..64],
                previous.side_to_move == Color::Black,
            );
            let before_move = if previous.side_to_move == target_position.side_to_move {
                saw_pass_then_placement = true;
                previous_oracle
                    .apply(PlyAction::Pass)
                    .expect("legal pass before the placement")
            } else {
                previous_oracle
            };
            assert!(
                before_move.moves().into_iter().any(|square| {
                    before_move
                        .apply(PlyAction::Play(square))
                        .unwrap()
                        .position()
                        == target_position
                }),
                "predecessor must replay to the exact target"
            );
        }
    }
    assert!(saw_pass_then_placement);
}

fn encoded_positions(positions: &[Position]) -> Vec<u8> {
    let mut bytes = b"OTHRBP01".to_vec();
    for position in positions {
        bytes.extend_from_slice(&position.board.player.to_le_bytes());
        bytes.extend_from_slice(&position.board.opponent.to_le_bytes());
        bytes.push(u8::from(position.side_to_move == Color::White));
    }
    bytes
}

#[test]
fn external_storage_merges_and_resumes_with_turn_in_each_record() {
    let directory = TestDirectory::new();
    let cfg = external_config(directory.path(), 4);
    let initial = ArrayPosition::initial().position();
    let target = ArrayPosition::initial()
        .apply(PlyAction::Play(19))
        .unwrap()
        .position();
    assert_eq!(
        parallel_retrospective_bfs(&cfg, &target, 4, &vec![initial]).unwrap(),
        SearchResult::Found
    );
    assert_eq!(read_target_board(&cfg.tmp_dir).unwrap(), target);
    assert_eq!(
        fs::read(cfg.tmp_dir.join("target_board.bin")).unwrap(),
        encoded_positions(&[target])
    );
    assert_eq!(
        fs::read(cfg.tmp_dir.join("r_5.bin")).unwrap(),
        encoded_positions(&[target])
    );
    assert_eq!(
        parallel_retrospective_bfs_resume(&cfg, 5, 4, &vec![initial]).unwrap(),
        SearchResult::Found
    );

    let other_turn = Position {
        side_to_move: Color::Black,
        ..target
    };
    let mut both = vec![target, other_turn];
    both.sort_unstable();
    let input = cfg.tmp_dir.join("merge_input.bin");
    let output = cfg.tmp_dir.join("merged.bin");
    fs::write(&input, encoded_positions(&both)).unwrap();
    assert_eq!(
        merge_sorted_bins(&[input.clone(), input], &output).unwrap(),
        2
    );
    assert_eq!(fs::read(output).unwrap(), encoded_positions(&both));

    let blocked = TestDirectory::new();
    let blocked_cfg = external_config(blocked.path(), 4);
    assert_eq!(
        sequential_retrospective_bfs(&blocked_cfg, &target, 4, &vec![initial]).unwrap(),
        SearchResult::Found
    );

    for malformed in [vec![0; 16], vec![0; 8], b"OTHRBP01x".to_vec()] {
        fs::write(cfg.tmp_dir.join("r_5.bin"), &malformed).unwrap();
        assert!(expand_layer_blocked(4, &cfg.tmp_dir, 2).is_err());
        fs::write(cfg.tmp_dir.join("target_board.bin"), &malformed).unwrap();
        assert!(read_target_board(&cfg.tmp_dir).is_err());
    }
    let mut invalid_turn = encoded_positions(&[target]);
    invalid_turn[24] = 2;
    fs::write(cfg.tmp_dir.join("target_board.bin"), invalid_turn).unwrap();
    assert!(read_target_board(&cfg.tmp_dir).is_err());
}

#[test]
fn all_seven_cli_modes_reject_missing_turn_and_preserve_position_output() {
    let initial = ArrayPosition::initial();
    let first = initial.apply(PlyAction::Play(19)).unwrap();
    let accepted = [initial.position(), first.position(), initial.position()];
    let rejected = initial.color_exchanged().position();
    let input_text = format!(
        "{}\n{}\n{}\n{}\n",
        format_position(accepted[0]),
        format_position(rejected),
        format_position(accepted[1]),
        format_position(accepted[2]),
    );
    let expected_ok = accepted.map(format_position).join("\n") + "\n";
    for mode in [
        "dfs",
        "dfs-move-ordering",
        "dfs-parallel",
        "gbfs-parallel",
        "bfs-inmemory-parallel",
        "bfs-external",
        "bfs-external-parallel",
    ] {
        let directory = TestDirectory::new();
        let input = directory.path().join("input.txt");
        fs::write(&input, &input_text).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_reverse_to_initial"));
        command
            .arg(mode)
            .arg(&input)
            .arg("--out-dir")
            .arg(directory.path())
            .args(["--discs", "4"])
            .env_remove("DISCS")
            .env_remove("MAX_NODES")
            .env_remove("TABLE_SIZE")
            .env_remove("RAYON_THREADS");
        if mode.starts_with("bfs-external") {
            command
                .args(["--jobs", "2", "--tmp-dir"])
                .arg(directory.path().join("tmp"));
        } else {
            command.args(["--max-nodes", "1000"]);
            if mode == "dfs-parallel" || mode == "gbfs-parallel" {
                command.args(["--threads", "1"]);
            }
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("reverse_OK.txt")).unwrap(),
            expected_ok,
            "{mode}"
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("reverse_NG.txt")).unwrap(),
            format!("{}\n", format_position(rejected)),
            "{mode}"
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("reverse_UNKNOWN.txt")).unwrap(),
            "",
            "{mode}"
        );

        fs::write(
            &input,
            format!("{}\n", &format_position(initial.position())[..64]),
        )
        .unwrap();
        assert!(
            !command.output().unwrap().status.success(),
            "{mode} must reject a missing turn"
        );
    }
}

#[test]
fn transposition_table_keeps_its_limit_cache_eviction_and_reuse() {
    let positions = support::enumerate(ArrayPosition::initial(), 6)
        .into_iter()
        .take(6)
        .map(ArrayPosition::position)
        .collect::<Vec<_>>();
    let mut table = Btable::new(4, 2);
    for &position in &positions[..4] {
        assert!(table.insert(position));
    }
    assert_eq!(table.len(), 4);
    assert!(table.insert(positions[4]));
    assert_eq!(table.len(), 5); // The separate insertion cache is retained.
    assert!(table.insert(positions[5]));
    assert_eq!(table.len(), 4); // The full cache is discarded at the table limit.
    for &position in &positions[..4] {
        assert!(!table.insert(position));
    }
    table.clear();
    assert_eq!(table.len(), 0);
    for &position in &positions[..4] {
        assert!(table.insert(position));
    }
    assert_eq!(table.len(), 4);
}
