#!/usr/bin/env python3
"""Collect stone statistics from Othello positions with an absolute turn."""

import argparse
import sys
from collections import Counter
from typing import Iterable, Optional, TextIO, List


def parse_position(text: str) -> str:
    """Validate all fields and return the 64 relative board cells."""
    if len(text) != 66 or text[64] != " " or text[65] not in ("b", "w"):
        raise ValueError("expected exactly 64 X/O/- cells, one space, and b or w")
    board = text[:64]
    if any(cell not in "XO-" for cell in board):
        raise ValueError("board cells must be X, O, or -")
    if any(board[square] == "-" for square in (27, 28, 35, 36)):
        raise ValueError("the centre four squares must be occupied")
    return board


def iter_boards(handle: TextIO) -> Iterable[str]:
    """Yield validated positions, retaining their order and absolute turns."""
    for line_number, raw_line in enumerate(handle, 1):
        text = raw_line.removesuffix("\n").removesuffix("\r")
        try:
            parse_position(text)
        except ValueError as error:
            raise ValueError(f"line {line_number}: {error}") from error
        yield text


def count_stones(position: str) -> int:
    """Return the stone count after validating the complete position."""
    board = parse_position(position)
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
        description="手番付きオセロ局面から石数の平均と分布を求めます。",
    )
    parser.add_argument(
        "path",
        nargs="?",
        help="手番付き局面を含むファイル。省略時は標準入力を読み取ります。",
    )
    parser.add_argument(
        "--stones",
        type=int,
        metavar="N",
        help="石数Nの局面を手番と重複を保って列挙します。",
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
