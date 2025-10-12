#!/usr/bin/env python3
"""Collect stone statistics from Othello boards described with O/X/- characters."""

import argparse
import sys
from collections import Counter
from typing import Iterable, Optional, TextIO, List


def iter_boards(handle: TextIO) -> Iterable[str]:
    """Yield cleaned, non-empty board strings from the given text handle."""
    for raw_line in handle:
        board = raw_line.strip()
        if not board:
            continue
        yield board


def count_stones(board: str) -> int:
    """Return the number of stones (O or X) on the board."""
    if any(cell not in ("O", "X", "-") for cell in board):
        raise ValueError(f"Unexpected character in board: {board!r}")
    return sum(cell in ("O", "X") for cell in board)


def summarize(
    handle: TextIO, target: Optional[int] = None
) -> tuple[int, Counter[int], List[str]]:
    """Process the handle, returning (board_count, stone_counter, matching_boards)."""
    counter = Counter()
    board_count = 0
    matches: List[str] = []
    for board in iter_boards(handle):
        stones = count_stones(board)
        counter[stones] += 1
        board_count += 1
        if target is not None and stones == target:
            matches.append(board)
    return board_count, counter, matches


def main() -> None:
    parser = argparse.ArgumentParser(
        description="オセロ盤面ファイルから石数の平均と分布を求めます。",
    )
    parser.add_argument(
        "path",
        nargs="?",
        help="盤面を含むテキストファイル。省略時は標準入力を読み取ります。",
    )
    parser.add_argument(
        "--stones",
        type=int,
        metavar="N",
        help="石数Nの盤面を列挙します。",
    )
    args = parser.parse_args()

    if args.path:
        with open(args.path, "r", encoding="utf-8") as handle:
            board_count, counter, matches = summarize(handle, args.stones)
    else:
        board_count, counter, matches = summarize(sys.stdin, args.stones)

    if board_count == 0:
        print("盤面が見つかりませんでした。")
        return

    total_stones = sum(stones * num_boards for stones, num_boards in counter.items())
    average = total_stones / board_count

    print(f"平均石数: {average:.6f}")
    print("石数ごとの盤面数:")
    for stones, num_boards in sorted(counter.items()):
        print(f"  {stones}: {num_boards}")

    if args.stones is not None:
        print(f"\n石数 {args.stones} の盤面一覧:")
        if matches:
            for board in matches:
                print(board)
        else:
            print("  該当する盤面がありません。")


if __name__ == "__main__":
    main()
