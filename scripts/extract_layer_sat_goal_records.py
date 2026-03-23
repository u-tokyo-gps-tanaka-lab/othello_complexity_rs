#!/usr/bin/env python3

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


GOAL_RE = re.compile(r"^goal\[(\d+)\]\s+([XO-]+)$")
SAT_RE = re.compile(r"^\[(\d+)\] SAT \(H=\d+\)$")
RECORD_RE = re.compile(r"^\[(\d+)\] Record:\s*(.*)$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Extract goal-board and SAT record pairs from layer_sat logs."
    )
    parser.add_argument(
        "inputs",
        nargs="+",
        type=Path,
        help="Input layer_sat log files.",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=Path("result/layer_sat_goal.txt"),
        help="Output text file path.",
    )
    return parser.parse_args()


def extract_pairs(path: Path) -> list[tuple[str, str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    goals: dict[int, str] = {}
    pairs: list[tuple[str, str]] = []
    pending_sat: int | None = None

    i = 0
    while i < len(lines):
        line = lines[i]

        goal_match = GOAL_RE.match(line)
        if goal_match:
            goal_id = int(goal_match.group(1))
            goals[goal_id] = goal_match.group(2)
            i += 1
            continue

        sat_match = SAT_RE.match(line)
        if sat_match:
            if pending_sat is not None:
                raise ValueError(
                    f"{path}: found SAT before matching Record for [{pending_sat}]"
                )
            pending_sat = int(sat_match.group(1))
            i += 1
            continue

        record_match = RECORD_RE.match(line)
        if record_match:
            record_id = int(record_match.group(1))
            if pending_sat == record_id:
                record = record_match.group(2).strip()
                if not record:
                    i += 1
                    if i >= len(lines):
                        raise ValueError(
                            f"{path}: missing record body for [{record_id}]"
                        )
                    record = lines[i].strip()

                try:
                    board = goals[record_id]
                except KeyError as exc:
                    raise ValueError(f"{path}: missing goal[{record_id}]") from exc

                pairs.append((board, record))
                pending_sat = None

        i += 1

    if pending_sat is not None:
        raise ValueError(f"{path}: missing Record for [{pending_sat}]")

    return pairs


def main() -> int:
    args = parse_args()
    all_pairs: list[tuple[str, str]] = []

    for input_path in args.inputs:
        all_pairs.extend(extract_pairs(input_path))

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8") as fh:
        for board, record in all_pairs:
            fh.write(f"{board},{record}\n")

    print(f"wrote {len(all_pairs)} line(s) to {args.output}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
