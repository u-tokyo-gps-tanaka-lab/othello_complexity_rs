// translated with ChatGPT 4o
/**
 * retrospective-dfs-reversi
 *
 * https://github.com/eukaryo/retrospective-dfs-reversi
 *
 * @date 2020
 * @author Hiroki Takizawa
 */

/// 盤面 `b` が 8 近傍で連結しているかを判定する関数。
/// 中央4マス(初期配置)が必ず含まれる前提です。
pub fn is_connected(b: u64) -> bool {
    let mut mark: u64 = 0x0000_0018_1800_0000u64;
    let mut old_mark: u64 = 0;

    // 中央 4 マスが存在しているか確認
    assert!((b & mark) == mark);

    // マークが更新されなくなるまでループ
    while mark != old_mark {
        old_mark = mark;
        let mut new_mark = mark;

        new_mark |= b & ((mark & 0xFEFE_FEFE_FEFE_FEFEu64) >> 1);
        new_mark |= b & ((mark & 0x7F7F_7F7F_7F7F_7F7Fu64) << 1);
        new_mark |= b & ((mark & 0xFFFF_FFFF_FFFF_FF00u64) >> 8);
        new_mark |= b & ((mark & 0x00FF_FFFF_FFFF_FFFFu64) << 8);
        new_mark |= b & ((mark & 0x7F7F_7F7F_7F7F_7F00u64) >> 7);
        new_mark |= b & ((mark & 0x00FE_FEFE_FEFE_FEFEu64) << 7);
        new_mark |= b & ((mark & 0xFEFE_FEFE_FEFE_FE00u64) >> 9);
        new_mark |= b & ((mark & 0x007F_7F7F_7F7F_7F7Fu64) << 9);

        mark = new_mark;
    }

    // 全ての石がマークされていれば連結とみなす
    mark == b
}
