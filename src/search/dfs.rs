use std::io;

use crate::othello::Position;
use crate::record::PlyAction;
use crate::search::{
    constraints::ReverseConstraints,
    core::{found, pass_predecessor, PlacementPredecessors, SearchNode, SearchOutcome},
    forward::ForwardTable,
    transposition::Btable,
};

/// Depth-first search over exact one-action predecessors.
/// Clear `visited` before each query; its allocated storage can be reused.
pub fn retrospective_search(
    goal: Position,
    table: &ForwardTable,
    visited: &mut Btable,
    node_limit: usize,
) -> io::Result<SearchOutcome> {
    search(goal, table, visited, node_limit, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::parse_position;
    use crate::othello::Color;
    use crate::record::replay;
    use crate::search::core::append_placement_predecessors;

    // The former eager work-stack implementation is deliberately retained in
    // tests to compare traversal, resource stops and witness paths, not only
    // the final reachable/unreachable classification.
    fn eager_reference(
        goal: Position,
        table: &ForwardTable,
        visited: &mut Btable,
        node_limit: usize,
        ordered: bool,
    ) -> io::Result<SearchOutcome> {
        enum Work {
            Visit(SearchNode),
            Placements(SearchNode, u64),
        }
        table.validate_goal(goal)?;
        if table.contains(goal) {
            return found(table, goal, &SearchNode::root(goal), 0);
        }
        if goal.board.popcount() <= table.discs() {
            return Ok(SearchOutcome::NotFound { nodes: 0 });
        }
        let constraints = ReverseConstraints::new(table, goal);
        let mut open = vec![Work::Visit(SearchNode::root(goal))];
        let mut nodes = 0;
        while let Some(work) = open.pop() {
            let (node, candidates) = match work {
                Work::Visit(node) => {
                    if table.contains(node.position) {
                        return found(table, goal, &node, nodes);
                    }
                    if node.position.board.popcount() <= table.discs()
                        || !visited.insert(node.position)
                    {
                        continue;
                    }
                    nodes += 1;
                    if nodes > node_limit {
                        return Ok(SearchOutcome::Unknown { nodes });
                    }
                    let Some(candidates) = constraints.candidates(node.position) else {
                        continue;
                    };
                    if let Some(previous) = pass_predecessor(node.position) {
                        let pass = node.preceding(previous, PlyAction::Pass);
                        open.push(Work::Placements(node, candidates));
                        open.push(Work::Visit(pass));
                        continue;
                    }
                    (node, candidates)
                }
                Work::Placements(node, candidates) => (node, candidates),
            };
            let mut previous = Vec::new();
            append_placement_predecessors(node.position, candidates, &mut previous);
            if ordered {
                crate::search::move_ordering::order_placements(&mut previous);
            }
            for (position, action) in previous.into_iter().rev() {
                open.push(Work::Visit(node.preceding(position, action)));
            }
        }
        Ok(SearchOutcome::NotFound { nodes })
    }

    fn compare(goal: Position, table: &ForwardTable, node_limit: usize, ordered: bool) {
        // A deliberately small cache also exercises forgetting visited states.
        let mut old_visited = Btable::new(32, 8);
        let mut new_visited = Btable::new(32, 8);
        let expected = eager_reference(goal, table, &mut old_visited, node_limit, ordered).unwrap();
        let actual = search(goal, table, &mut new_visited, node_limit, ordered).unwrap();
        assert_eq!(actual.result(), expected.result(), "goal={goal:?}");
        assert_eq!(actual.nodes(), expected.nodes(), "goal={goal:?}");
        if let (
            SearchOutcome::Found { record: actual, .. },
            SearchOutcome::Found {
                record: expected, ..
            },
        ) = (actual, expected)
        {
            assert_eq!(actual.actions, expected.actions, "goal={goal:?}");
            actual.verify().unwrap();
        }
    }

    #[test]
    fn lazy_dfs_preserves_eager_paths_counts_and_resource_stops() {
        let table = ForwardTable::new(5, None).unwrap();
        let reachable = ForwardTable::new(8, None).unwrap();
        let mut goals: Vec<_> = reachable.positions().collect();
        goals.sort_unstable();
        let stride = (goals.len() / 64).max(1);
        for goal in goals.into_iter().step_by(stride) {
            let opposite_turn = Position {
                board: goal.board.swapped(),
                side_to_move: goal.side_to_move.opposite(),
            };
            for goal in [goal, opposite_turn] {
                for ordered in [false, true] {
                    for limit in [0, 1, 8, usize::MAX] {
                        compare(goal, &table, limit, ordered);
                    }
                }
            }
        }
        compare(
            Position {
                side_to_move: Color::White,
                ..Position::initial()
            },
            &table,
            0,
            false,
        );
    }

    #[test]
    fn lazy_dfs_preserves_pass_first_and_terminal_paths() {
        let start =
            parse_position("OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO w")
                .unwrap();
        let actions = [PlyAction::Play(15), PlyAction::Pass, PlyAction::Play(55)];
        let table = ForwardTable::from_start(start, start.board.popcount(), None).unwrap();
        for goal in replay(start, &actions).unwrap() {
            for ordered in [false, true] {
                for limit in [0, 1, 8, 128] {
                    compare(goal, &table, limit, ordered);
                }
            }
        }
    }
}

pub(crate) fn search(
    goal: Position,
    table: &ForwardTable,
    visited: &mut Btable,
    node_limit: usize,
    ordered: bool,
) -> io::Result<SearchOutcome> {
    // Keep only a continuation for each ancestor. The ordinary DFS generates
    // one placement at a time, so a successful first child or a resource stop
    // never pays for the remaining Cartesian product of unflip choices.
    struct Frame {
        pass: Option<Position>,
        placements: PlacementPredecessors,
        ranked: Option<std::vec::IntoIter<(Position, PlyAction)>>,
    }

    impl Frame {
        fn next(&mut self, ordered: bool) -> Option<(Position, PlyAction)> {
            if let Some(position) = self.pass.take() {
                return Some((position, PlyAction::Pass));
            }
            if !ordered {
                return self.placements.next();
            }
            // Sorting necessarily inspects every child, but defer it until
            // after the pass branch and retain only one shared action path.
            self.ranked
                .get_or_insert_with(|| {
                    let mut previous: Vec<_> = self.placements.by_ref().collect();
                    crate::search::move_ordering::order_placements(&mut previous);
                    previous.into_iter()
                })
                .next()
        }
    }

    table.validate_goal(goal)?;
    if table.contains(goal) {
        return found(table, goal, &SearchNode::root(goal), 0);
    }
    if goal.board.popcount() <= table.discs() {
        return Ok(SearchOutcome::NotFound { nodes: 0 });
    }
    let constraints = ReverseConstraints::new(table, goal);
    // Every placement removes one stone and two legal passes cannot be
    // consecutive. Thus at most two frames per remaining stone are needed.
    let max_depth = 2 * (goal.board.popcount() - table.discs()) as usize + 1;
    let mut open: Vec<Frame> = Vec::with_capacity(max_depth);
    let mut reverse_actions = Vec::with_capacity(max_depth);
    let mut position = goal;
    let mut nodes = 0;
    loop {
        if table.contains(position) {
            return found(
                table,
                goal,
                &SearchNode {
                    position,
                    reverse_actions,
                },
                nodes,
            );
        }
        if position.board.popcount() > table.discs() && visited.insert(position) {
            nodes += 1;
            if nodes > node_limit {
                return Ok(SearchOutcome::Unknown { nodes });
            }
            if let Some(candidates) = constraints.candidates(position) {
                open.push(Frame {
                    pass: pass_predecessor(position),
                    placements: PlacementPredecessors::new(position, candidates),
                    ranked: None,
                });
            }
        }
        loop {
            let Some(depth) = open.len().checked_sub(1) else {
                return Ok(SearchOutcome::NotFound { nodes });
            };
            reverse_actions.truncate(depth);
            if let Some((previous, action)) = open[depth].next(ordered) {
                reverse_actions.push(action);
                position = previous;
                break;
            }
            open.pop();
        }
    }
}
