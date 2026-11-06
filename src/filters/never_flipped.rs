use crate::{
    filters::occupancy::{check_occupancy, occupancy_order},
    othello::{
        east, ne, north, nw, pseudo_mobility_bits, se, south, sw, west, Direction, CENTER_MASK,
    },
};

const FORWARD_SHIFTS: [fn(u64) -> u64; 4] = [east, north, ne, nw];
const BACKWARD_SHIFTS: [fn(u64) -> u64; 4] = [west, south, sw, se];

/// 与えた局面から、一度も返っていないと証明できる石を返す。
pub fn never_flipped_discs(player: u64, opponent: u64) -> u64 {
    let occupied = player | opponent;
    let supported: [u64; 4] = std::array::from_fn(|axis| {
        FORWARD_SHIFTS[axis](occupied) & BACKWARD_SHIFTS[axis](occupied)
    });
    let supported_on_other_axes = [
        supported[1] | supported[2] | supported[3],
        supported[0] | supported[2] | supported[3],
        supported[0] | supported[1] | supported[3],
        supported[0] | supported[1] | supported[2],
    ];
    let mut never_flipped = 0;
    loop {
        let mut next = occupied;
        for axis in 0..4 {
            let forward = FORWARD_SHIFTS[axis];
            let backward = BACKWARD_SHIFTS[axis];
            let decided = never_flipped | (occupied & !supported_on_other_axes[axis]);
            let differing_neighbour = ((forward(player & decided) | backward(player & decided))
                & opponent)
                | ((forward(opponent & decided) | backward(opponent & decided)) & player);
            next &= !supported[axis] | differing_neighbour;
        }
        if next == never_flipped {
            return never_flipped;
        }
        never_flipped = next;
    }
}

/// 指定された返っていない石を、矛盾なく置ける順序が存在するかを調べる。
fn placement_order_exists(player: u64, opponent: u64, mut never_flipped: u64) -> bool {
    loop {
        let remaining_player = player & never_flipped;
        let remaining_opponent = opponent & never_flipped;
        let enclosing = (pseudo_mobility_bits(remaining_player, remaining_opponent)
            & remaining_player)
            | (pseudo_mobility_bits(remaining_opponent, remaining_player) & remaining_opponent);
        let removable = never_flipped & !enclosing & !CENTER_MASK;
        if removable == 0 {
            return never_flipped & !CENTER_MASK == 0;
        }
        never_flipped &= !removable;
    }
}

/// 一度も返っていない石を、矛盾なく置ける順序が存在するかを調べる。
pub fn check_placement_order(player: u64, opponent: u64) -> bool {
    placement_order_exists(player, opponent, never_flipped_discs(player, opponent))
}

/// 各マスから各方向へ、隣とその先の2マスを表す。どちらかが盤外なら0。
static ADJACENT_AND_BEYOND_MASKS: [[u64; 8]; 64] = adjacent_and_beyond_masks();

const fn adjacent_and_beyond_masks() -> [[u64; 8]; 64] {
    let directions = Direction::all();
    let mut masks = [[0_u64; 8]; 64];
    let mut square = 0;
    while square < 64 {
        let x = (square & 7) as i32;
        let y = (square >> 3) as i32;
        let mut direction = 0;
        while direction < 8 {
            let (dx, dy) = directions[direction].to_offset();
            let adjacent_x = x + dx;
            let adjacent_y = y + dy;
            let beyond_x = adjacent_x + dx;
            let beyond_y = adjacent_y + dy;
            if (adjacent_x as u32) < 8
                && (adjacent_y as u32) < 8
                && (beyond_x as u32) < 8
                && (beyond_y as u32) < 8
            {
                let adjacent = (adjacent_y * 8 + adjacent_x) as usize;
                let beyond = (beyond_y * 8 + beyond_x) as usize;
                masks[square][direction] = (1_u64 << adjacent) | (1_u64 << beyond);
            }
            direction += 1;
        }
        square += 1;
    }
    masks
}

/// 占有配置だけで決まる、各石を置いたときの反転候補方向。
pub(super) struct FlipDirections {
    directions: [u8; 64],
}

impl FlipDirections {
    /// 占有配置から、各石を置いたときの反転候補方向を作る。
    ///
    /// # 前提条件
    /// `check_occupancy(occupied)` が真であり、`order` が
    /// `occupancy_order(occupied)` の結果でなければならない。
    /// このとき、各石を最初に説明する2マスはその石を除いても説明できるため、
    /// 中央以外の候補方向は空にならない。
    pub(super) fn new(occupied: u64, order: &[u64; 64]) -> Self {
        let mut directions = [0u8; 64];
        let mut discs = occupied & !CENTER_MASK;
        while discs != 0 {
            let square = discs.trailing_zeros() as usize;
            discs &= discs - 1;
            let discs_not_confirmed_later = occupied & order[square];

            // 隣とその先が、この石より後に置かれたと確定しない方向を残す。
            for (direction, &adjacent_and_beyond) in
                ADJACENT_AND_BEYOND_MASKS[square].iter().enumerate()
            {
                if adjacent_and_beyond != 0
                    && discs_not_confirmed_later & adjacent_and_beyond == adjacent_and_beyond
                {
                    directions[square] |= 1_u8 << direction;
                }
            }
        }
        Self { directions }
    }

    /// 着手順序と最終的な石の色が、幾何条件と両立するかを調べる。
    ///
    /// # 前提条件
    /// この値が `player | opponent` に対して作られ、`never_flipped` が
    /// `never_flipped_discs(player, opponent)` の結果でなければならない。
    pub(super) fn consistent_with(&self, player: u64, opponent: u64, never_flipped: u64) -> bool {
        placement_order_exists(player, opponent, never_flipped)
            && self.placements_explained(player, opponent, never_flipped)
    }

    fn placements_explained(&self, player: u64, opponent: u64, never_flipped: u64) -> bool {
        for placed_color in [player, opponent] {
            let mut discs = placed_color & never_flipped & !CENTER_MASK;
            while discs != 0 {
                let square = discs.trailing_zeros() as usize;
                discs &= discs - 1;
                if !self.has_possible_flip_direction(square, placed_color, never_flipped) {
                    return false;
                }
            }
        }
        true
    }

    // この石を置いたとき、盤上のいずれかの方向で反転が可能かを判定する。
    fn has_possible_flip_direction(
        &self,
        square: usize,
        placed_color: u64,
        never_flipped: u64,
    ) -> bool {
        let different_color_never_flipped = never_flipped & !placed_color;
        let mut directions = self.directions[square];
        while directions != 0 {
            let direction = directions.trailing_zeros() as usize;
            directions &= directions - 1;

            // 隣接マスとその先のマスに一度も反転していない異色の石がなければ、この方向での反転は可能なままである。
            if ADJACENT_AND_BEYOND_MASKS[square][direction] & different_color_never_flipped == 0 {
                return true;
            }
        }
        false
    }
}

/// 反転整合性の必要条件を検査する。
///
/// 占有到達性が満たされない場合も到達不能であるため `false` を返す。
/// 呼出し元で占有到達性を検査しておく必要はない。
pub fn check_never_flipped(player: u64, opponent: u64) -> bool {
    let occupied = player | opponent;
    if !check_occupancy(occupied) {
        return false;
    }
    let order = occupancy_order(occupied);
    let never_flipped = never_flipped_discs(player, opponent);
    FlipDirections::new(occupied, &order).consistent_with(player, opponent, never_flipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::othello::{flip, Position};
    use crate::record::PlyAction;
    use crate::search::moves::successors;
    use rand::{Rng, SeedableRng};

    #[test]
    fn complete_games_satisfy_the_flip_conditions() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x7465_6d70_6f72_616c);
        let mut passes = 0;
        for (games, choose_fewest_flips) in [(64, false), (1, true)] {
            for _ in 0..games {
                let mut position = Position::initial();
                let mut historically_flipped = 0_u64;
                loop {
                    let next = successors(position);
                    if next.is_empty() {
                        break;
                    }
                    let (target, action) = if choose_fewest_flips {
                        next.iter()
                            .copied()
                            .min_by_key(|candidate| match candidate.1 {
                                PlyAction::Play(endpoint) => {
                                    flip(endpoint, position.board.player, position.board.opponent)
                                        .count_ones()
                                }
                                PlyAction::Pass => 0,
                            })
                            .unwrap()
                    } else {
                        next[rng.random_range(0..next.len())]
                    };
                    match action {
                        PlyAction::Play(endpoint) => {
                            historically_flipped |=
                                flip(endpoint, position.board.player, position.board.opponent);
                        }
                        PlyAction::Pass => passes += 1,
                    }
                    position = target;
                    let occupied = position.board.player | position.board.opponent;
                    let order = occupancy_order(occupied);
                    let geometry = FlipDirections::new(occupied, &order);
                    let never_flipped =
                        never_flipped_discs(position.board.player, position.board.opponent);
                    assert_eq!(
                        historically_flipped & never_flipped,
                        0,
                        "historically flipped disc marked as never flipped: position={position:?}",
                    );
                    assert!(geometry.consistent_with(
                        position.board.player,
                        position.board.opponent,
                        never_flipped,
                    ));
                }
            }
        }
        assert!(passes > 0, "deterministic complete games must cover passes");
    }
}
