//! Forward reachability with exact positions and explicit pass edges.

use std::collections::{HashMap, VecDeque};
use std::io;

use crate::othello::{validate_board, Color, Direction, Position};
use crate::record::PlyAction;
use crate::search::core::successors;

/// Reachable positions at every stone count through the meeting boundary.
/// Parent edges retain a witness even for passes at the boundary and terminal
/// positions. A goal with fewer stones is looked up at its own stone count.
pub struct ForwardTable {
    start: Position,
    discs: u32,
    target: Option<Position>,
    parents: HashMap<Position, Option<(Position, PlyAction)>>,
}

impl ForwardTable {
    pub fn new(discs: u32, target: Option<Position>) -> io::Result<Self> {
        Self::from_start(Position::initial(), discs, target)
    }

    pub(crate) fn from_start(
        start: Position,
        discs: u32,
        target: Option<Position>,
    ) -> io::Result<Self> {
        validate_board(&start.board).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid start: {error:?}"),
            )
        })?;
        if let Some(goal) = target {
            validate_board(&goal.board).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid target: {error:?}"),
                )
            })?;
        }
        if !(start.board.popcount()..=64).contains(&discs) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "meeting discs must be between the starting count and 64",
            ));
        }
        let discs = target.map_or(discs, |goal| discs.min(goal.board.popcount()));
        let mut parents = HashMap::from([(start, None)]);
        let mut open = VecDeque::from([start]);
        while let Some(position) = open.pop_front() {
            for (next, action) in successors(position) {
                if next.board.popcount() > discs {
                    continue;
                }
                if target.is_some_and(|goal| !compatible(next, goal)) {
                    continue;
                }
                if let std::collections::hash_map::Entry::Vacant(entry) = parents.entry(next) {
                    entry.insert(Some((position, action)));
                    open.push_back(next);
                }
            }
        }
        Ok(Self {
            start,
            discs,
            target,
            parents,
        })
    }

    /// A target-pruned table proves absence only for the target used to build it.
    pub(crate) fn validate_goal(&self, goal: Position) -> io::Result<()> {
        validate_board(&goal.board).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid goal: {error:?}"),
            )
        })?;
        if self.target.is_some_and(|target| target != goal) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "forward table was pruned for another goal",
            ));
        }
        Ok(())
    }

    pub fn start(&self) -> Position {
        self.start
    }
    pub fn discs(&self) -> u32 {
        self.discs
    }
    pub fn contains(&self, position: Position) -> bool {
        self.parents.contains_key(&position)
    }
    pub fn len(&self) -> usize {
        self.parents.len()
    }
    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
    }
    pub fn positions(&self) -> impl Iterator<Item = Position> + '_ {
        self.parents.keys().copied()
    }

    pub fn actions_to(&self, mut position: Position) -> Option<Vec<PlyAction>> {
        let mut actions = Vec::new();
        while let Some((parent, action)) = *self.parents.get(&position)? {
            actions.push(action);
            position = parent;
        }
        actions.reverse();
        Some(actions)
    }
}

/// Occupied squares that cannot flip before the target occupancy is reached.
fn stable_discs(occupied: u64, target: u64) -> u64 {
    let mut stable = 0;
    let mut remaining = occupied;
    while remaining != 0 {
        let index = remaining.trailing_zeros();
        remaining &= remaining - 1;
        let (x, y) = ((index % 8) as i32, (index / 8) as i32);
        let mut can_flip = false;
        for direction in Direction::all() {
            let (dx, dy) = direction.to_offset();
            let (mut u, mut v) = (x + dx, y + dy);
            while (0..8).contains(&u) && (0..8).contains(&v) && occupied & (1 << (8 * v + u)) != 0 {
                u += dx;
                v += dy;
            }
            let (other_x, other_y) = (x - dx, y - dy);
            if (0..8).contains(&u)
                && (0..8).contains(&v)
                && target & (1 << (8 * v + u)) != 0
                && (0..8).contains(&other_x)
                && (0..8).contains(&other_y)
                && target & (1 << (8 * other_y + other_x)) != 0
            {
                can_flip = true;
                break;
            }
        }
        if !can_flip {
            stable |= 1 << index;
        }
    }
    stable
}

/// Necessary occupancy and immutable-color conditions for continuing to target.
/// Also used when deriving a goal-specific frontier from a shared forward table.
pub(crate) fn compatible(position: Position, target: Position) -> bool {
    let occupied = position.board.player | position.board.opponent;
    let target_occupied = target.board.player | target.board.opponent;
    if occupied & target_occupied != occupied {
        return false;
    }
    let stable = stable_discs(occupied, target_occupied);
    let black = |p: Position| {
        if p.side_to_move == Color::Black {
            p.board.player
        } else {
            p.board.opponent
        }
    };
    black(position) & stable == black(target) & stable
}
