#!/usr/bin/env python3
"""Summarize paired bench_reverse_search runs, including an estimated fallback.

The fallback is a calculation over independently timed runs. It does not claim
that the two-stage search itself was executed or that its timing was measured.
"""

import argparse
import csv
import itertools
import json
import math
import sys
from collections import Counter, defaultdict
from dataclasses import asdict, dataclass
from pathlib import Path


@dataclass(frozen=True, order=True)
class Variant:
    algorithm: str
    width: int
    threads: int
    max_nodes: int
    budget_counts: str

    def label(self):
        width = f", width {self.width}" if self.algorithm != "gbfs" else ""
        return (
            f"{self.algorithm}{width}, {self.max_nodes} {self.budget_counts}, "
            f"{self.threads} thread(s)"
        )


def read_runs(path):
    required = {
        "case", "stones", "known_reachable", "algorithm", "width", "repeat",
        "threads", "max_nodes", "budget_counts", "result", "search_seconds",
        "generated", "expanded", "peak_frontier", "forward_discs",
        "forward_positions", "seed", "position",
    }
    grouped = defaultdict(dict)
    with path.open(newline="", encoding="utf-8") as source:
        reader = csv.DictReader(source)
        missing = required - set(reader.fieldnames or ())
        if missing:
            raise ValueError(f"missing CSV columns: {', '.join(sorted(missing))}")
        for line, raw in enumerate(reader, 2):
            try:
                row = dict(raw)
                for name in (
                    "case", "stones", "repeat", "threads", "max_nodes",
                    "generated", "expanded", "peak_frontier", "forward_discs",
                    "forward_positions",
                ):
                    row[name] = int(raw[name])
                    if row[name] < 0:
                        raise ValueError(f"{name} must not be negative")
                if min(row["case"], row["repeat"], row["threads"]) == 0:
                    raise ValueError("case, repeat, and threads must be positive")
                row["search_seconds"] = float(raw["search_seconds"])
                if not math.isfinite(row["search_seconds"]) or row["search_seconds"] < 0:
                    raise ValueError("search_seconds must be finite and nonnegative")
                if row["result"] not in {"OK", "NG", "UNKNOWN"}:
                    raise ValueError("result must be OK, NG, or UNKNOWN")
                if row["known_reachable"] not in {"true", "false"}:
                    raise ValueError("known_reachable must be true or false")
                if row["known_reachable"] == "true" and row["result"] == "NG":
                    raise ValueError("a known reachable position was classified NG")
                variant = Variant(
                    raw["algorithm"], int(raw["width"] or 0), row["threads"],
                    row["max_nodes"], raw["budget_counts"],
                )
                if variant.algorithm == "gbfs":
                    if variant.width != 0 or variant.budget_counts != "registered":
                        raise ValueError("GBFS requires no width and a registered-state budget")
                elif variant.width <= 0 or variant.budget_counts != "generated":
                    raise ValueError("beam requires a positive width and generated-state budget")
                key = row["case"], row["repeat"]
                if key in grouped[variant]:
                    raise ValueError(f"duplicate case/repeat {key} for {variant.label()}")
                grouped[variant][key] = row
            except (TypeError, ValueError) as error:
                raise ValueError(f"line {line}: {error}") from error
    baselines = [variant for variant in grouped if variant.algorithm == "gbfs"]
    if len(baselines) != 1:
        raise ValueError("CSV must contain exactly one GBFS baseline configuration")
    baseline = baselines[0]
    reference = grouped[baseline]
    cases = {key[0] for key in reference}
    repeats = {key[1] for key in reference}
    if cases != set(range(1, max(cases) + 1)) or repeats != set(range(1, max(repeats) + 1)):
        raise ValueError("baseline case and repeat indices must start at 1 without gaps")
    expected = set(itertools.product(cases, repeats))
    identity = (
        "position", "stones", "known_reachable", "forward_discs", "forward_positions", "seed",
    )
    # The corpus must also be identical across repeats, not merely across methods.
    for key, row in reference.items():
        first = reference.get((key[0], 1))
        if first is None or any(row[name] != first[name] for name in identity):
            raise ValueError(f"baseline corpus changes across repeats for case {key[0]}")
    for variant, runs in grouped.items():
        if set(runs) != expected:
            missing = sorted(expected - set(runs))[:5]
            extra = sorted(set(runs) - expected)[:5]
            raise ValueError(
                f"incomplete paired corpus for {variant.label()}: "
                f"missing case/repeat {missing}, extra {extra}"
            )
        for key, row in runs.items():
            other = reference[key]
            if any(row[name] != other[name] for name in identity):
                raise ValueError(f"corpus or forward table differs for {variant.label()}, {key}")
            if row["result"] != "UNKNOWN" and other["result"] != "UNKNOWN":
                if row["result"] != other["result"]:
                    raise ValueError(f"contradictory OK/NG results for case/repeat {key}")
    return grouped, baseline


def summarize(rows):
    outcomes = Counter(row["result"] for row in rows)
    return {
        "cases": len({row["case"] for row in rows}),
        "runs": len(rows),
        "ok": outcomes["OK"],
        "ng": outcomes["NG"],
        "unknown": outcomes["UNKNOWN"],
        "resolved": outcomes["OK"] + outcomes["NG"],
        "generated": sum(row["generated"] for row in rows),
        "expanded": sum(row["expanded"] for row in rows),
        "search_seconds": math.fsum(row["search_seconds"] for row in rows),
        "peak_frontier": max(row["peak_frontier"] for row in rows),
    }


def make_report(grouped, baseline):
    reference = grouped[baseline]
    stone_counts = sorted({row["stones"] for row in reference.values()})
    direct, fallback = [], []
    for stones in [None, *stone_counts]:
        keys = sorted(key for key, row in reference.items() if stones is None or row["stones"] == stones)
        base_summary = summarize([reference[key] for key in keys])
        for variant in [baseline, *sorted(variant for variant in grouped if variant != baseline)]:
            rows = [grouped[variant][key] for key in keys]
            direct.append({"variant": asdict(variant), "stones": stones, **summarize(rows)})
            if variant == baseline:
                continue
            combined, gbfs_calls = [], 0
            for key, row in zip(keys, rows):
                item = dict(row)
                if row["result"] == "UNKNOWN":
                    gbfs_calls += 1
                    other = reference[key]
                    item["result"] = other["result"]
                    for name in ("generated", "expanded", "search_seconds"):
                        item[name] += other[name]
                    item["peak_frontier"] = max(item["peak_frontier"], other["peak_frontier"])
                combined.append(item)
            totals = summarize(combined)
            ratios = {
                name: totals[name] / base_summary[name] if base_summary[name] else None
                for name in ("generated", "expanded", "search_seconds")
            }
            fallback.append({
                "variant": asdict(variant), "stones": stones, **totals,
                "gbfs_calls": gbfs_calls, "ratios_to_gbfs": ratios,
            })
    return {
        "notes": [
            "Counts of OK, NG, UNKNOWN, and resolved refer to runs, including all repeats.",
            "Direct timings are measured searches; forward-table construction is excluded.",
            "Fallback runs GBFS only after beam UNKNOWN; beam NG is already a proof.",
            "Fallback outcomes and costs are calculated from paired independent runs. "
            "The two-stage search was not executed or timed as a pipeline by this script.",
            "Fallback time is the sum of separately measured search times, not a pipeline measurement.",
            "Budget units differ: GBFS bounds registered positions; beam bounds generated candidates.",
            "peak_frontier counts retained/open positions, not process memory or cache storage.",
        ],
        "baseline": asdict(baseline),
        "cases": len({key[0] for key in reference}),
        "repeats": len({key[1] for key in reference}),
        "stone_counts": stone_counts,
        "direct": direct,
        "estimated_fallback": fallback,
    }


def markdown(report):
    lines = ["# Reverse-search benchmark", "", f"Corpus: {report['cases']} cases, {report['repeats']} repeats.", ""]
    lines.extend(f"- {note}" for note in report["notes"])
    for stones in [None, *report["stone_counts"]]:
        label = "All stone counts" if stones is None else f"{stones} stones"
        lines += ["", f"## {label}", "", "### Measured individual searches", "",
                  "| Method | OK | NG | UNKNOWN | Generated | Expanded | Seconds | Peak frontier |",
                  "|---|---:|---:|---:|---:|---:|---:|---:|"]
        for row in report["direct"]:
            if row["stones"] != stones:
                continue
            name = Variant(**row["variant"]).label().replace("|", "\\|")
            lines.append(
                f"| {name} | {row['ok']} | {row['ng']} | {row['unknown']} | "
                f"{row['generated']:,} | {row['expanded']:,} | {row['search_seconds']:.6f} | {row['peak_frontier']:,} |"
            )
        lines += ["", "### Estimated beam followed by GBFS on UNKNOWN", "",
                  "| Beam method | OK | NG | UNKNOWN | GBFS calls | Generated | Expanded | Estimated seconds | Generated / GBFS | Expanded / GBFS | Seconds / GBFS | Peak frontier |",
                  "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
        for row in report["estimated_fallback"]:
            if row["stones"] != stones:
                continue
            name = Variant(**row["variant"]).label().replace("|", "\\|")
            ratios = [row["ratios_to_gbfs"][name] for name in ("generated", "expanded", "search_seconds")]
            formatted = " | ".join("n/a" if value is None else f"{value:.4f}" for value in ratios)
            lines.append(
                f"| {name} | {row['ok']} | {row['ng']} | {row['unknown']} | {row['gbfs_calls']} | "
                f"{row['generated']:,} | {row['expanded']:,} | {row['search_seconds']:.6f} | "
                f"{formatted} | {row['peak_frontier']:,} |"
            )
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", type=Path, help="complete output of bench_reverse_search")
    parser.add_argument("--format", choices=("markdown", "json"), default="markdown")
    parser.add_argument("--output", type=Path, help="write the summary here instead of stdout")
    args = parser.parse_args()
    try:
        grouped, baseline = read_runs(args.csv)
        report = make_report(grouped, baseline)
        output = json.dumps(report, ensure_ascii=False, indent=2) + "\n" if args.format == "json" else markdown(report)
        if args.output:
            args.output.write_text(output, encoding="utf-8")
        else:
            sys.stdout.write(output)
    except (OSError, ValueError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
