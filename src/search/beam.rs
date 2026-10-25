//! Deterministic reverse beam search with bounded candidate storage.
//!
//! Width pruning is deliberately incomplete: failure after any width cut is
//! `Unknown`, never a proof of unreachability. Every success is replayed by
//! `found`, just as in the exhaustive search engines.

use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::io;

use clap::ValueEnum;

use crate::filters::impossible_edges::check_virtual_edge_patterns;
use crate::hash::CustomHash;
use crate::othello::{get_last_moves, get_moves, Color, Position};
use crate::record::PlyAction;
use crate::search::{
    constraints::ReverseConstraints,
    core::{found, pass_predecessor, PlacementPredecessors, SearchNode, SearchOutcome},
    forward::{compatible, ForwardTable},
    parallel_gbfs::placement_predecessor_count,
};

/// These scores order candidates only; none is a necessary condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum BeamHeuristic {
    /// Prefer fewer placement predecessors, as in GBFS.
    Branching,
    /// Prefer a larger total number of legal moves for the two players.
    Mobility,
    /// Prefer same-color adjacency for both colors, using bit operations.
    Coherence,
    /// Prefer color agreement with reachable, occupancy-compatible frontier states.
    Frontier,
    /// Combine same-color adjacency with the number of viable last-move squares.
    Continuation,
}

impl fmt::Display for BeamHeuristic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Branching => "branching",
            Self::Mobility => "mobility",
            Self::Coherence => "coherence",
            Self::Frontier => "frontier",
            Self::Continuation => "continuation",
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BeamConfig {
    /// Maximum open positions, or positions per layer in the layered variant.
    /// Must be nonzero.
    pub width: usize,
    /// Maximum generated predecessor positions, including reverse passes.
    /// Duplicate and rejected candidates count; the initial roots do not.
    pub max_nodes: usize,
    pub heuristic: BeamHeuristic,
}

impl Default for BeamConfig {
    fn default() -> Self {
        Self {
            width: 32,
            max_nodes: 1_000_000,
            heuristic: BeamHeuristic::Mobility,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BeamStats {
    /// Predecessor positions generated, before deduplication and filtering.
    pub generated: usize,
    /// Positions whose placement predecessors were enumerated.
    pub expanded: usize,
    /// Admissions to the frontier, including roots and later evictions.
    pub retained: usize,
    /// Maximum open plus active positions: at most width + 1 for best-first,
    /// or 2 * width for the layered variant. Excludes the fixed caches/table.
    pub peak_frontier: usize,
    /// At least one candidate was discarded solely to enforce the width.
    pub pruned: bool,
}

#[derive(Debug)]
pub struct BeamReport {
    /// `nodes` has the same value as `stats.generated`.
    pub outcome: SearchOutcome,
    pub stats: BeamStats,
}

/// Count each undirected king-adjacency edge once, without row wraparound.
fn same_color_edges(stones: u64) -> u32 {
    const NOT_A_FILE: u64 = 0xfefe_fefe_fefe_fefe;
    const NOT_H_FILE: u64 = 0x7f7f_7f7f_7f7f_7f7f;
    (stones & ((stones & NOT_H_FILE) << 1)).count_ones()
        + (stones & (stones << 8)).count_ones()
        + (stones & ((stones & NOT_H_FILE) << 9)).count_ones()
        + (stones & ((stones & NOT_A_FILE) << 7)).count_ones()
}

/// Smaller priorities are better. Entry::key ranks stone count first;
/// this score breaks ties between positions with the same count.
fn priority(position: Position, heuristic: BeamHeuristic, frontier: &[(u64, u64)]) -> u128 {
    let board = position.board;
    match heuristic {
        BeamHeuristic::Branching => {
            let count = placement_predecessor_count(position);
            if count == 0 {
                u128::MAX
            } else {
                count as u128
            }
        }
        BeamHeuristic::Mobility => {
            let moves = get_moves(board.player, board.opponent).count_ones()
                + get_moves(board.opponent, board.player).count_ones();
            u128::MAX - u128::from(moves)
        }
        BeamHeuristic::Coherence => u128::MAX - coherence(position),
        BeamHeuristic::Continuation => {
            continuation_priority(position, get_last_moves(board.opponent, board.player))
        }
        BeamHeuristic::Frontier => {
            let occupied = board.player | board.opponent;
            let black = absolute_black(position);
            let mut support = 0_u128;
            for &(mask, colors) in frontier {
                if mask & occupied == mask {
                    let mismatches = ((black ^ colors) & mask).count_ones();
                    // Exact fixed-point form of sum 2^(-mismatches). Integer
                    // addition makes table iteration order irrelevant.
                    support = support.saturating_add(1_u128 << (64 - mismatches));
                }
            }
            u128::MAX - support
        }
    }
}

fn coherence(position: Position) -> u128 {
    let player = u128::from(same_color_edges(position.board.player)) + 1;
    let opponent = u128::from(same_color_edges(position.board.opponent)) + 1;
    player * opponent
}

fn continuation_priority(position: Position, candidates: u64) -> u128 {
    u128::MAX - coherence(position) * (u128::from(candidates.count_ones()) + 1)
}

fn absolute_black(position: Position) -> u64 {
    match position.side_to_move {
        Color::Black => position.board.player,
        Color::White => position.board.opponent,
    }
}

struct Entry {
    priority: u128,
    node: SearchNode,
    candidates: u64,
}

impl Entry {
    fn key(&self) -> (u32, u128, Position) {
        (
            self.node.position.board.popcount(),
            self.priority,
            self.node.position,
        )
    }
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

struct Beam {
    entries: BTreeSet<Entry>,
    positions: HashSet<Position, CustomHash>,
    width: usize,
}

impl Beam {
    fn new(width: usize) -> Self {
        // Do not preallocate an arbitrary user-supplied width.
        Self {
            entries: BTreeSet::new(),
            positions: HashSet::default(),
            width,
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn accepts(&self, key: (u32, u128, Position)) -> bool {
        self.len() < self.width || key < self.entries.last().unwrap().key()
    }

    fn insert(&mut self, entry: Entry, stats: &mut BeamStats) {
        if self.len() == self.width {
            let removed = self.entries.pop_last().unwrap();
            self.positions.remove(&removed.node.position);
            stats.pruned = true;
        }
        self.positions.insert(entry.node.position);
        self.entries.insert(entry);
        stats.retained += 1;
    }

    fn take_best(&mut self) -> Option<Entry> {
        let entry = self.entries.pop_first()?;
        self.positions.remove(&entry.node.position);
        Some(entry)
    }

    fn finish(self) -> Vec<Entry> {
        // Ascending priority and exact-position tie-break make runs repeatable.
        self.entries.into_iter().collect()
    }
}

struct Context<'a> {
    goal: Position,
    table: &'a ForwardTable,
    constraints: ReverseConstraints,
    config: &'a BeamConfig,
    standard_start: bool,
    frontier: Vec<(u64, u64)>,
    expanded: ExpandedCache,
}

/// A bounded cache of fully expanded exact positions. A hash collision evicts
/// its previous key, causing recomputation rather than suppressing a new state.
/// Fixed hashing keeps both eviction and search order reproducible.
struct ExpandedCache {
    slots: Vec<Cell<Option<Position>>>,
}

impl ExpandedCache {
    fn new(capacity: usize) -> Self {
        debug_assert!(capacity == 0 || capacity.is_power_of_two());
        Self {
            slots: vec![Cell::new(None); capacity],
        }
    }

    fn slot(&self, position: Position) -> usize {
        let turn = u64::from(position.side_to_move == Color::White);
        let key = position.board.player.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ position.board.opponent.rotate_left(29)
            ^ turn;
        let mixed = (key ^ (key >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        let mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((mixed ^ (mixed >> 31)) as usize) & (self.slots.len() - 1)
    }

    fn contains(&self, position: Position) -> bool {
        !self.slots.is_empty() && self.slots[self.slot(position)].get() == Some(position)
    }

    fn insert(&self, position: Position) {
        if !self.slots.is_empty() {
            self.slots[self.slot(position)].set(Some(position));
        }
    }
}

impl Context<'_> {
    fn expand(
        &self,
        entry: &Entry,
        next: &mut Beam,
        stats: &mut BeamStats,
        active: usize,
    ) -> io::Result<Option<SearchOutcome>> {
        if stats.generated >= self.config.max_nodes {
            return Ok(Some(SearchOutcome::Unknown {
                nodes: stats.generated,
            }));
        }
        stats.expanded += 1;
        let mut previous = PlacementPredecessors::new(entry.node.position, entry.candidates);
        loop {
            // Check before enumerating, including large Cartesian products.
            if stats.generated >= self.config.max_nodes {
                return Ok(Some(SearchOutcome::Unknown {
                    nodes: stats.generated,
                }));
            }
            let Some((position, action)) = previous.next() else {
                break;
            };
            stats.generated += 1;
            if let Some(outcome) = self.offer(
                position,
                || entry.node.preceding(position, action),
                next,
                stats,
            )? {
                return Ok(Some(outcome));
            }
            stats.peak_frontier = stats.peak_frontier.max(active + next.len());
            if let Some(before_pass) = pass_predecessor(position) {
                if stats.generated >= self.config.max_nodes {
                    return Ok(Some(SearchOutcome::Unknown {
                        nodes: stats.generated,
                    }));
                }
                stats.generated += 1;
                if let Some(outcome) = self.offer(
                    before_pass,
                    || {
                        entry
                            .node
                            .preceding(position, action)
                            .preceding(before_pass, PlyAction::Pass)
                    },
                    next,
                    stats,
                )? {
                    return Ok(Some(outcome));
                }
                stats.peak_frontier = stats.peak_frontier.max(active + next.len());
            }
        }
        Ok(None)
    }

    /// Build/copy a witness only if the candidate meets or enters the beam.
    fn offer(
        &self,
        position: Position,
        make_node: impl FnOnce() -> SearchNode,
        beam: &mut Beam,
        stats: &mut BeamStats,
    ) -> io::Result<Option<SearchOutcome>> {
        // A witness wins even when its heuristic would fall outside the beam.
        if position.board.popcount() <= self.table.discs() {
            return if self.table.contains(position) {
                found(self.table, self.goal, &make_node(), stats.generated).map(Some)
            } else {
                Ok(None)
            };
        }
        if beam.positions.contains(&position) || self.expanded.contains(position) {
            return Ok(None);
        }
        if self.standard_start
            && !check_virtual_edge_patterns(position.board.player, position.board.opponent)
        {
            return Ok(None);
        }
        let mut rank = priority(position, self.config.heuristic, &self.frontier);
        // All retained entries have passed the necessary conditions. During
        // one expansion the cutoff can only improve. A worse candidate cannot
        // enter the beam;
        // avoid its expensive occupancy/seg3 analysis. Conservatively remember
        // the cut even if that untested candidate might itself be impossible.
        if !beam.accepts((position.board.popcount(), rank, position)) {
            stats.pruned = true;
            return Ok(None);
        }
        let Some(candidates) = self.constraints.candidates(position) else {
            return Ok(None);
        };
        // Reverse-pass predecessors are offered separately, including when
        // the position after that pass has no placement predecessor itself.
        if candidates == 0 {
            return Ok(None);
        }
        if self.config.heuristic == BeamHeuristic::Continuation {
            // The cheap unfiltered last-move mask above is a superset of the
            // viable mask. Its score is an upper bound, so a failed early cut
            // remains valid. Rank retained entries by the exact viable count.
            rank = continuation_priority(position, candidates);
            if !beam.accepts((position.board.popcount(), rank, position)) {
                stats.pruned = true;
                return Ok(None);
            }
        }
        beam.insert(
            Entry {
                priority: rank,
                node: make_node(),
                candidates,
            },
            stats,
        );
        Ok(None)
    }
}

fn report(outcome: SearchOutcome, stats: BeamStats) -> BeamReport {
    BeamReport { outcome, stats }
}

/// Search backwards with bounded layer storage and a hard generation budget.
///
/// A layer removes one placement. The legal state before a forced pass is
/// included in that same layer, with both actions preserved in the witness.
/// Only retained positions are deduplicated: an evicted position has no proof
/// attached and is never entered into an exhaustive-search transposition table.
/// Memory for candidates is O(width), and paths use O(width * path length).
pub fn retrospective_layered_beam_search(
    goal: Position,
    table: &ForwardTable,
    config: &BeamConfig,
) -> io::Result<BeamReport> {
    search(goal, table, config, false)
}

/// Bounded best-first beam: prefer fewer remaining stones, then the selected
/// heuristic, and expand a promising descendant without sweeping a full layer.
/// The open set contains at most `width` positions across all stone counts.
/// A fixed 4096-entry cache avoids re-expanding recently completed positions;
/// collisions evict a complete key and can only cause additional work. Neither
/// width pruning nor cache eviction is a proof that a position is unreachable.
pub fn retrospective_beam_search(
    goal: Position,
    table: &ForwardTable,
    config: &BeamConfig,
) -> io::Result<BeamReport> {
    search(goal, table, config, true)
}

fn search(
    goal: Position,
    table: &ForwardTable,
    config: &BeamConfig,
    best_first: bool,
) -> io::Result<BeamReport> {
    if config.width == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "beam width must be nonzero",
        ));
    }
    table.validate_goal(goal)?;
    let mut stats = BeamStats::default();
    let root = SearchNode::root(goal);
    if table.contains(goal) {
        return found(table, goal, &root, 0).map(|outcome| report(outcome, stats));
    }
    if goal.board.popcount() <= table.discs() {
        return Ok(report(SearchOutcome::NotFound { nodes: 0 }, stats));
    }
    let context = Context {
        goal,
        table,
        constraints: ReverseConstraints::new(table, goal),
        config,
        standard_start: table.start() == Position::initial(),
        frontier: if config.heuristic == BeamHeuristic::Frontier {
            table
                .positions()
                .filter(|position| {
                    position.board.popcount() == table.discs() && compatible(*position, goal)
                })
                .map(|position| {
                    (
                        position.board.player | position.board.opponent,
                        absolute_black(position),
                    )
                })
                .collect()
        } else {
            Vec::new()
        },
        expanded: ExpandedCache::new(if best_first { 4096 } else { 0 }),
    };
    let mut roots = Beam::new(config.width);
    if let Some(outcome) = context.offer(goal, || root.clone(), &mut roots, &mut stats)? {
        return Ok(report(outcome, stats));
    }
    if let Some(previous) = pass_predecessor(goal) {
        if let Some(outcome) = context.offer(
            previous,
            || root.preceding(previous, PlyAction::Pass),
            &mut roots,
            &mut stats,
        )? {
            return Ok(report(outcome, stats));
        }
    }
    stats.peak_frontier = roots.len();
    if best_first {
        while let Some(entry) = roots.take_best() {
            if let Some(outcome) = context.expand(&entry, &mut roots, &mut stats, 1)? {
                return Ok(report(outcome, stats));
            }
            context.expanded.insert(entry.node.position);
        }
        return Ok(exhausted(stats));
    }
    let mut current = roots.finish();
    while !current.is_empty() {
        let mut next = Beam::new(config.width);
        for entry in &current {
            if let Some(outcome) = context.expand(entry, &mut next, &mut stats, current.len())? {
                return Ok(report(outcome, stats));
            }
        }
        current = next.finish();
    }
    Ok(exhausted(stats))
}

fn exhausted(stats: BeamStats) -> BeamReport {
    let outcome = if stats.pruned {
        SearchOutcome::Unknown {
            nodes: stats.generated,
        }
    } else {
        SearchOutcome::NotFound {
            nodes: stats.generated,
        }
    };
    report(outcome, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::parse_position;
    use crate::othello::{Board, Color, CENTER_MASK};
    use crate::record::replay;
    use crate::search::core::{successors, SearchResult};

    #[test]
    fn adjacency_counts_each_edge_once_and_does_not_wrap_rows() {
        assert_eq!(same_color_edges(0), 0);
        assert_eq!(same_color_edges(u64::MAX), 210);
        assert_eq!(same_color_edges(CENTER_MASK), 6);
        assert_eq!(same_color_edges((1 << 7) | (1 << 8)), 0);
        assert_eq!(same_color_edges((1 << 0) | (1 << 15)), 0);
        assert_eq!(same_color_edges((1 << 7) | (1 << 16)), 0);
    }

    #[test]
    fn wide_beam_agrees_with_complete_forward_enumeration() {
        let table = ForwardTable::new(4, None).unwrap();
        let reachable = ForwardTable::new(7, None).unwrap();
        // Flip turns as well as checking all reachable early positions. At
        // these depths, the full forward table supplies an exact oracle.
        for position in reachable.positions() {
            for goal in [
                position,
                Position {
                    side_to_move: position.side_to_move.opposite(),
                    ..position
                },
            ] {
                for best_first in [false, true] {
                    for &heuristic in BeamHeuristic::value_variants() {
                        let config = BeamConfig {
                            width: 10_000,
                            max_nodes: 100_000,
                            heuristic,
                        };
                        let report = search(goal, &table, &config, best_first).unwrap();
                        assert!(!report.stats.pruned);
                        assert_eq!(
                            report.outcome.result() == SearchResult::Found,
                            reachable.contains(goal)
                        );
                        assert_ne!(report.outcome.result(), SearchResult::Unknown);
                        if let SearchOutcome::Found { record, .. } = report.outcome {
                            record.verify().unwrap();
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn generation_budget_and_width_are_enforced() {
        let table = ForwardTable::new(4, None).unwrap();
        let goal = successors(successors(Position::initial())[0].0)[0].0;
        for limit in [0, 1] {
            let report = retrospective_beam_search(
                goal,
                &table,
                &BeamConfig {
                    width: 1,
                    max_nodes: limit,
                    ..BeamConfig::default()
                },
            )
            .unwrap();
            assert_eq!(report.outcome.result(), SearchResult::Unknown);
            assert!(report.stats.generated <= limit);
            assert!(report.stats.peak_frontier <= 2);
        }
        assert!(retrospective_beam_search(
            goal,
            &table,
            &BeamConfig {
                width: 0,
                ..BeamConfig::default()
            }
        )
        .is_err());
    }

    #[test]
    fn width_cut_exhaustion_is_unknown_even_for_a_reachable_goal() {
        let start =
            parse_position("-------------X-----XX------XO-----OOO--------------------------- b")
                .unwrap();
        let goal = replay(start, &[PlyAction::Play(37), PlyAction::Play(6)]).unwrap()[2];
        let table = ForwardTable::from_start(start, 8, None).unwrap();
        let narrow = retrospective_beam_search(
            goal,
            &table,
            &BeamConfig {
                width: 1,
                heuristic: BeamHeuristic::Coherence,
                ..BeamConfig::default()
            },
        )
        .unwrap();
        assert!(narrow.stats.pruned);
        assert_eq!(narrow.outcome.result(), SearchResult::Unknown);
        assert!(narrow.stats.generated < BeamConfig::default().max_nodes);
        let wide = retrospective_beam_search(
            goal,
            &table,
            &BeamConfig {
                width: 64,
                heuristic: BeamHeuristic::Coherence,
                ..BeamConfig::default()
            },
        )
        .unwrap();
        let SearchOutcome::Found { record, .. } = wide.outcome else {
            panic!("wide beam lost a known witness");
        };
        record.verify().unwrap();
    }

    #[test]
    fn forced_pass_and_terminal_keep_replayable_witnesses() {
        let start =
            parse_position("OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO w")
                .unwrap();
        let trace = replay(
            start,
            &[PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)],
        )
        .unwrap();
        let table = ForwardTable::from_start(start, 62, None).unwrap();
        for goal in [trace[2], trace[3]] {
            for &heuristic in BeamHeuristic::value_variants() {
                for best_first in [false, true] {
                    let report = search(
                        goal,
                        &table,
                        &BeamConfig {
                            width: 64,
                            heuristic,
                            ..BeamConfig::default()
                        },
                        best_first,
                    )
                    .unwrap();
                    let SearchOutcome::Found { record, .. } = report.outcome else {
                        panic!("missing pass witness: {report:?}");
                    };
                    record.verify().unwrap();
                    assert!(record.actions.contains(&PlyAction::Pass));
                    assert!(report.stats.peak_frontier <= 128);
                }
            }
        }
    }

    #[test]
    fn expanded_cache_collisions_and_turns_only_cause_recomputation() {
        // A single slot forces every key to collide, including its other turn.
        let cache = ExpandedCache::new(1);
        let start = Position::initial();
        let other_turn = Position {
            side_to_move: start.side_to_move.opposite(),
            ..start
        };
        cache.insert(start);
        assert!(cache.contains(start));
        assert!(!cache.contains(other_turn));
        cache.insert(other_turn);
        assert!(!cache.contains(start));
        assert!(cache.contains(other_turn));
        let disabled = ExpandedCache::new(0);
        disabled.insert(start);
        assert!(!disabled.contains(start));
    }

    #[test]
    fn frontier_score_uses_absolute_colors_and_only_contained_occupancies() {
        let initial = Position::initial();
        let frontier = [
            (CENTER_MASK, initial.board.player),
            (CENTER_MASK, initial.board.opponent),
            (CENTER_MASK | 1, initial.board.player),
        ];
        let expected = u128::MAX - (1_u128 << 64) - (1_u128 << 60);
        assert_eq!(
            priority(initial, BeamHeuristic::Frontier, &frontier),
            expected
        );
        let changed_view = Position {
            board: initial.board.swapped(),
            side_to_move: initial.side_to_move.opposite(),
        };
        assert_eq!(
            priority(changed_view, BeamHeuristic::Frontier, &frontier),
            expected
        );
    }

    #[test]
    fn custom_start_and_zero_placement_reverse_pass_are_preserved() {
        let empty = (1_u64 << 0) | (1_u64 << 20);
        let black = (1_u64 << 1) | (1_u64 << 19);
        let start = Position {
            board: Board::new(!(empty | black), black),
            side_to_move: Color::White,
        };
        let trace = replay(
            start,
            &[PlyAction::Play(0), PlyAction::Pass, PlyAction::Play(20)],
        )
        .unwrap();
        let table = ForwardTable::from_start(start, 62, Some(trace[3])).unwrap();
        // Preserve every viable predecessor here: this test isolates the pass
        // handling from the intentional incompleteness of a narrow beam.
        let report = retrospective_beam_search(
            trace[3],
            &table,
            &BeamConfig {
                width: usize::MAX,
                ..BeamConfig::default()
            },
        )
        .unwrap();
        let SearchOutcome::Found { record, .. } = report.outcome else {
            panic!("missing witness: {report:?}");
        };
        record.verify().unwrap();
        assert!(record.actions.contains(&PlyAction::Pass));
        assert!(retrospective_beam_search(trace[2], &table, &BeamConfig::default()).is_err());
    }
}
