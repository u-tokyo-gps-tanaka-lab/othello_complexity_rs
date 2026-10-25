#!/usr/bin/env python3
"""Independently replay exact Othello certificates using an absolute-color array.

This verifier deliberately does not call the Rust binary or import its move,
capture, pass, or bitboard implementation. It accepts the headerless six-column
ReachabilityRecord CSV emitted by reverse_search and bench_reverse_search.

With --positions, each record must address that exact 1-based input occurrence.
With --benchmark-csv, every OK row must have one independently replayed record;
per-input multiplicities include all algorithms and repetitions in that CSV.
No conclusion about an UNKNOWN or NG position is inferred from this check.
"""

from __future__ import annotations

import argparse
import collections
import csv
import hashlib
import io
import json
from pathlib import Path
import sys


INITIAL = "---------------------------OX------XO--------------------------- b"
CENTER = (27, 28, 35, 36)
DIRECTIONS = tuple(
    (dx, dy)
    for dy in (-1, 0, 1)
    for dx in (-1, 0, 1)
    if (dx, dy) != (0, 0)
)


def parse_position(text: str) -> tuple[tuple[int, ...], int]:
    """Convert X/O relative to the stated turn into absolute black/white."""
    if len(text) != 66 or text[64] != " " or text[65] not in "bw":
        raise ValueError("position must contain 64 cells, a space, and b or w")
    turn = 1 if text[65] == "b" else 2
    encoding = {"-": 0, "X": turn, "O": 3 - turn}
    try:
        board = tuple(encoding[cell] for cell in text[:64])
    except KeyError as error:
        raise ValueError("position contains a character other than X/O/-") from error
    if any(board[square] == 0 for square in CENTER):
        raise ValueError("position is missing an occupied center square")
    return board, turn


def captures(board: tuple[int, ...], turn: int, square: int) -> list[int]:
    if not 0 <= square < 64 or board[square] != 0:
        return []
    result: list[int] = []
    for dx, dy in DIRECTIONS:
        x, y = square % 8 + dx, square // 8 + dy
        line: list[int] = []
        while 0 <= x < 8 and 0 <= y < 8:
            index = y * 8 + x
            if board[index] == 3 - turn:
                line.append(index)
            else:
                if board[index] == turn and line:
                    result.extend(line)
                break
            x += dx
            y += dy
    return result


def has_move(board: tuple[int, ...], turn: int) -> bool:
    return any(captures(board, turn, square) for square in range(64))


def replay(start: str, actions: str) -> tuple[tuple[int, ...], int, int, int]:
    board, turn = parse_position(start)
    placements = 0
    passes = 0
    for ply, token in enumerate(actions.split(" ") if actions else (), 1):
        if token == "ps":
            if has_move(board, turn):
                raise ValueError(f"ply {ply}: pass despite an available placement")
            if not has_move(board, 3 - turn):
                raise ValueError(f"ply {ply}: pass after terminal position")
            passes += 1
        else:
            if len(token) != 2 or token[0] not in "ABCDEFGH" or token[1] not in "12345678":
                raise ValueError(f"ply {ply}: expected A1 through H8 or ps")
            square = 8 * (int(token[1]) - 1) + ord(token[0]) - ord("A")
            flipped = captures(board, turn, square)
            if not flipped:
                raise ValueError(f"ply {ply}: illegal placement {token}")
            changed = list(board)
            changed[square] = turn
            for index in flipped:
                changed[index] = turn
            board = tuple(changed)
            placements += 1
        turn = 3 - turn
    return board, turn, placements, passes


def verify_fields(
    fields: list[str], positions: list[str] | None
) -> tuple[int, int, int]:
    if len(fields) != 6:
        raise ValueError("record must have exactly six fields")
    raw_index, start, goal, mode, transform, actions = fields
    if not raw_index.isascii() or not raw_index.isdigit() or int(raw_index) < 1:
        raise ValueError("input index must be a positive decimal integer")
    index = int(raw_index)
    if mode != "exact" or transform != "identity":
        raise ValueError("this independent verifier accepts exact/identity records only")
    if start != INITIAL:
        raise ValueError("this experiment requires the standard black-to-move start")
    if positions is not None:
        if index > len(positions) or goal != positions[index - 1]:
            raise ValueError("record goal does not match its indexed input occurrence")
    expected_board, expected_turn = parse_position(goal)
    actual_board, actual_turn, placements, passes = replay(start, actions)
    if (actual_board, actual_turn) != (expected_board, expected_turn):
        raise ValueError("replayed absolute board/turn does not match the exact goal")
    return index, placements, passes


def read_positions(path: Path, snapshot: bytes) -> list[str]:
    positions = snapshot.decode("utf-8").splitlines()
    for line, text in enumerate(positions, 1):
        try:
            parse_position(text)
        except ValueError as error:
            raise ValueError(f"{path}:{line}: {error}") from error
    return positions


def benchmark_counts(
    path: Path, positions: list[str] | None, snapshot: bytes
) -> collections.Counter:
    expected: collections.Counter = collections.Counter()
    with io.StringIO(snapshot.decode("utf-8"), newline="") as stream:
        reader = csv.DictReader(stream)
        if not {"case", "position", "result"}.issubset(reader.fieldnames or ()):
            raise ValueError("benchmark CSV is missing case, position, or result")
        for line, row in enumerate(reader, 2):
            try:
                index = int(row["case"])
                if index < 1:
                    raise ValueError("case index is not positive")
                parse_position(row["position"])
                if positions is not None and (
                    index > len(positions) or row["position"] != positions[index - 1]
                ):
                    raise ValueError("benchmark position does not match its input index")
                if row["result"] not in {"OK", "NG", "UNKNOWN"}:
                    raise ValueError("unknown benchmark result")
                if row["result"] == "OK":
                    expected[index] += 1
            except ValueError as error:
                raise ValueError(f"{path}:{line}: {error}") from error
    return expected


def sha256(snapshot: bytes) -> str:
    return hashlib.sha256(snapshot).hexdigest()


def self_test() -> None:
    # At white's turn X denotes white, so construct a known first-move board
    # explicitly rather than importing the production formatter.
    first = "-------------------O-------OO------OX--------------------------- w"
    fields = ["1", INITIAL, first, "exact", "identity", "D3"]
    verify_fields(fields, [first])
    verify_fields(["1", INITIAL, INITIAL, "exact", "identity", ""], [INITIAL])

    rejected = [
        ["1", INITIAL, first[:-1] + "b", "exact", "identity", "D3"],
        ["1", INITIAL, first, "exact", "identity", "D3 D3"],
        ["1", INITIAL, INITIAL, "exact", "identity", "ps"],
        ["1", INITIAL, INITIAL, "equivalence", "identity", ""],
        ["2", INITIAL, first, "exact", "identity", "D3"],
    ]
    for fields in rejected:
        try:
            verify_fields(fields, [first])
        except ValueError:
            pass
        else:
            raise ValueError(f"independent verifier accepted forged fixture: {fields!r}")

    # A dense known fixture has a forced pass between H2 and H7.
    dense = "OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO w"
    board, turn, placements, passes = replay(dense, "H2 ps H7")
    if placements != 2 or passes != 1 or has_move(board, turn) or has_move(board, 3 - turn):
        raise ValueError("forced-pass/terminal fixture did not replay as expected")
    for actions in ("H2 H7", "H2 ps H7 ps"):
        try:
            replay(dense, actions)
        except ValueError:
            pass
        else:
            raise ValueError(f"independent verifier accepted illegal pass fixture: {actions}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("records", type=Path, nargs="?")
    parser.add_argument("--positions", type=Path)
    parser.add_argument("--benchmark-csv", type=Path)
    parser.add_argument("--output", type=Path, help="optional JSON verification report")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.benchmark_csv and not args.positions:
        parser.error("--benchmark-csv requires --positions to bind each OK to its exact input")
    if args.self_test:
        self_test()
    if args.records is None:
        if args.self_test:
            print(json.dumps({"self_test": "passed"}))
            return
        parser.error("records is required unless only --self-test is requested")
    inputs = list(dict.fromkeys(
        path for path in (args.records, args.positions, args.benchmark_csv) if path
    ))
    if args.output and args.output.resolve() in {path.resolve() for path in inputs}:
        parser.error("verification output must not overwrite an input file")
    # Parse and hash the same immutable bytes. Reopening a path for its hash
    # after replay would identify different bytes if another process replaced
    # the file during verification.
    snapshots = {path: path.read_bytes() for path in inputs}
    positions = read_positions(args.positions, snapshots[args.positions]) if args.positions else None
    actual: collections.Counter = collections.Counter()
    placements = passes = 0
    with io.StringIO(snapshots[args.records].decode("utf-8"), newline="") as stream:
        for line, fields in enumerate(csv.reader(stream), 1):
            try:
                index, moves, skips = verify_fields(fields, positions)
            except ValueError as error:
                raise ValueError(f"{args.records}:{line}: {error}") from error
            actual[index] += 1
            placements += moves
            passes += skips
    if args.benchmark_csv:
        expected = benchmark_counts(args.benchmark_csv, positions, snapshots[args.benchmark_csv])
        if actual != expected:
            raise ValueError(
                "certificate multiplicities do not cover all benchmark OK rows: "
                f"missing={dict(expected - actual)}, extra={dict(actual - expected)}"
            )
    report = {
        "verifier": "independent-absolute-array-v1",
        "input_snapshot_policy": "each input read once; parsing and hashing use the same bytes",
        "records_sha256": sha256(snapshots[args.records]),
        "verified_records": sum(actual.values()),
        "distinct_input_indices": len(actual),
        "placements_replayed": placements,
        "passes_replayed": passes,
        "standard_start": True,
        "mode": "exact",
        "positions_sha256": sha256(snapshots[args.positions]) if args.positions else None,
        "benchmark_sha256": sha256(snapshots[args.benchmark_csv]) if args.benchmark_csv else None,
        "all_benchmark_ok_records_covered": args.benchmark_csv is not None,
    }
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    sys.stdout.write(rendered)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, csv.Error) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
