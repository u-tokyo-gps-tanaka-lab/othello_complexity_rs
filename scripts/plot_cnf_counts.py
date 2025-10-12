#!/usr/bin/env python3
"""
Generate scatter plots from CNF counts data without loading the entire file into memory.

Usage:
    python scripts/plot_cnf_counts.py --input /path/to/result/cnf_counts.txt --output-base plot
"""

import argparse
from pathlib import Path

import matplotlib.pyplot as plt


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Plot vars vs clauses grouped by SAT answer."
    )
    parser.add_argument(
        "--input",
        required=True,
        type=Path,
        help="Path to cnf_counts.txt file.",
    )
    parser.add_argument(
        "--output-base",
        type=Path,
        help="Base path for output images. Defaults to <input> without suffix.",
    )
    parser.add_argument(
        "--dpi",
        type=int,
        default=150,
        help="Figure dots per inch resolution.",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()

    input_path = args.input
    if not input_path.is_file():
        raise FileNotFoundError(f"Input file does not exist: {input_path}")

    output_base = args.output_base or input_path.with_suffix("")
    true_points: tuple[list[int], list[int]] = ([], [])
    false_points: tuple[list[int], list[int]] = ([], [])

    with input_path.open("r", encoding="utf-8") as fh:
        for raw_line in fh:
            line = raw_line.strip()
            if not line:
                continue
            segments = [segment.strip() for segment in line.split(",")]
            record = {}
            for segment in segments:
                if "=" not in segment:
                    continue
                key, value = segment.split("=", 1)
                record[key.strip()] = value.strip()

            try:
                answer = record["ans"].lower()
                vars_count = int(record["vars"])
                clauses_count = int(record["clauses"])
            except (KeyError, ValueError) as err:
                raise ValueError(f"Failed to parse line: {line}") from err

            target = true_points if answer == "true" else false_points
            target[0].append(vars_count)
            target[1].append(clauses_count)

    if not true_points[0] and not false_points[0]:
        raise ValueError("No records parsed from input file.")

    all_x = true_points[0] + false_points[0]
    all_y = true_points[1] + false_points[1]
    x_bounds = (min(all_x), max(all_x))
    y_bounds = (min(all_y), max(all_y))
    base_name = output_base.stem if output_base.suffix else output_base.name
    output_dir = output_base.parent if output_base.parent != Path("") else Path(".")

    def save_scatter(
        points: tuple[list[int], list[int]], label: str, color: str, suffix: str
    ) -> None:
        if not points[0]:
            print(f"Skipping {label} plot because there are no points.")
            return
        fig, ax = plt.subplots(figsize=(8, 6))
        ax.scatter(
            points[0],
            points[1],
            s=12,
            alpha=0.6,
            label=label,
            color=color,
        )
        ax.set_xlim(x_bounds)
        ax.set_ylim(y_bounds)
        ax.set_xlabel("vars")
        ax.set_ylabel("clauses")
        ax.set_title(f"CNF Vars vs Clauses ({label})")
        ax.legend()
        ax.grid(True, linestyle="--", linewidth=0.5, alpha=0.4)
        fig.tight_layout()
        output_path = output_dir / f"{base_name}_{suffix}.png"
        fig.savefig(output_path, dpi=args.dpi)
        print(f"Scatter plot saved to {output_path}")

    save_scatter(false_points, "ans = false", "#d95f02", "ans_false")
    save_scatter(true_points, "ans = true", "#1b9e77", "ans_true")


if __name__ == "__main__":
    main()
