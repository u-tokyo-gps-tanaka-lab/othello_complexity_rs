#!/usr/bin/env python3
"""Generate a Markdown stone-count distribution table from board files."""

import argparse
import sys
from collections import OrderedDict
from pathlib import Path
from typing import Iterable


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

try:
    from stone_stats import summarize  # type: ignore
except ImportError as exc:  # pragma: no cover - guard for misconfigured PYTHONPATH
    raise SystemExit(
        "Failed to import stone_stats. Run this script from the repository root."
    ) from exc


def iter_stone_counts(counters: Iterable[dict[int, int]]) -> list[int]:
    """Return sorted list of stone counts present across all counters."""
    stones = set()
    for counter in counters:
        stones.update(counter.keys())
    return sorted(stones)


def build_table(
    stone_counts: Iterable[int],
    stats: "OrderedDict[str, tuple[int, dict[int, int]]]",
) -> list[str]:
    """Return Markdown table lines (without leading/trailing blank lines)."""
    headers = ["Stones", *stats.keys()]
    header_line = "| " + " | ".join(headers) + " |"
    divider_line = "| " + " | ".join(["---"] * len(headers)) + " |"

    lines = [header_line, divider_line]
    for stone in stone_counts:
        row = [str(stone)]
        for _, (_, counter) in stats.items():
            row.append(str(counter.get(stone, 0)))
        lines.append("| " + " | ".join(row) + " |")
    return lines


def load_stats(paths: list[Path]) -> "OrderedDict[str, tuple[int, dict[int, int]]]":
    """Read each path and collect board counts and per-stone frequencies."""
    stats: "OrderedDict[str, tuple[int, dict[int, int]]]" = OrderedDict()
    for path in paths:
        with path.open("r", encoding="utf-8") as handle:
            board_count, counter, _ = summarize(handle)
        stats[path.name] = (board_count, dict(counter))
    return stats


def generate_markdown(
    stats: "OrderedDict[str, tuple[int, dict[int, int]]]", title: str
) -> str:
    """Return complete Markdown document for the collected statistics."""
    stone_counts = iter_stone_counts(counter for _, counter in stats.values())
    lines = [title, ""]
    lines.extend(build_table(stone_counts, stats))
    lines.append("")
    lines.append("Totals")
    for name, (boards, _) in stats.items():
        lines.append(f"- {name}: {boards} boards")
    lines.append("")
    return "\n".join(lines)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="指定したテキストファイルの石数分布をMarkdown表に出力します。"
    )
    parser.add_argument(
        "inputs",
        metavar="FILE",
        nargs="+",
        help="盤面を含むテキストファイル（複数指定可）。引数の順序で列を並べます。",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        required=True,
        help="Markdownを書き出すパス。",
    )
    parser.add_argument(
        "--title",
        default="# Stone Count Distribution",
        help="Markdown冒頭に出力するタイトル（既定: '# Stone Count Distribution'）。",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    input_paths = [Path(p) for p in args.inputs]
    missing = [str(p) for p in input_paths if not p.exists()]
    if missing:
        missing_list = ", ".join(missing)
        raise SystemExit(f"Input file not found: {missing_list}")

    stats = load_stats(input_paths)
    markdown = generate_markdown(stats, args.title)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(markdown, encoding="utf-8")


if __name__ == "__main__":
    main()
