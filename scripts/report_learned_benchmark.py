#!/usr/bin/env python3
"""Report repeated learned-search benchmarks and apply the prospective model rule.

Fallback times below are sums of independently measured searches. Use
bench_reverse_pipeline.py for actual command-line pipeline wall times.
"""

import argparse
import hashlib
import json
import math
import statistics
from collections import Counter
from dataclasses import asdict
from datetime import datetime, timezone
from pathlib import Path

from summarize_reverse_bench import read_runs


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def summarize_repetitions(runs):
    repeats = sorted({repeat for _, repeat in runs})
    totals = []
    signatures = []
    for repeat in repeats:
        rows = [row for (_, r), row in sorted(runs.items()) if r == repeat]
        totals.append({
            "repeat": repeat,
            "ok": sum(row["result"] == "OK" for row in rows),
            "ng": sum(row["result"] == "NG" for row in rows),
            "unknown": sum(row["result"] == "UNKNOWN" for row in rows),
            "generated": sum(row["generated"] for row in rows),
            "expanded": sum(row["expanded"] for row in rows),
            "search_seconds": math.fsum(row["search_seconds"] for row in rows),
            "model_evaluations": sum(row["model_evaluations"] for row in rows),
            "model_inference_seconds": math.fsum(row["model_inference_seconds"] for row in rows),
            "peak_frontier": max(row["peak_frontier"] for row in rows),
        })
        signatures.append([(row["case"], row["result"], row["generated"], row["expanded"])
                           for row in rows])
    return {
        "per_repeat": totals,
        "outcomes_and_search_counts_identical_across_repeats": all(
            signature == signatures[0] for signature in signatures
        ),
        "median_search_seconds": statistics.median(row["search_seconds"] for row in totals),
        "median_model_inference_seconds": statistics.median(
            row["model_inference_seconds"] for row in totals
        ),
    }


def report(path):
    grouped, baseline = read_runs(path)
    reference = grouped[baseline]
    base_summary = summarize_repetitions(reference)
    variants = []
    for variant, runs in sorted(grouped.items()):
        first = next(iter(runs.values()))
        direct = summarize_repetitions(runs)
        item = {
            "variant": asdict(variant),
            "model_path": first.get("model_path", ""),
            "model_hidden_size": int(first["model_hidden_size"]) if first.get("model_hidden_size") else None,
            "direct": direct,
        }
        if variant != baseline:
            combined = {}
            for key, row in runs.items():
                value = dict(row)
                if row["result"] == "UNKNOWN":
                    value["result"] = reference[key]["result"]
                    for metric in ("search_seconds", "generated", "expanded"):
                        value[metric] += reference[key][metric]
                    value["peak_frontier"] = max(value["peak_frontier"], reference[key]["peak_frontier"])
                combined[key] = value
            estimated = summarize_repetitions(combined)
            estimated["time_kind"] = "sum of independently timed searches; not an executed pipeline"
            estimated["median_time_ratio_to_gbfs"] = (
                estimated["median_search_seconds"] / base_summary["median_search_seconds"]
                if base_summary["median_search_seconds"] else None
            )
            for metric in ("generated", "expanded"):
                base_total = sum(row[metric] for row in reference.values())
                estimated[f"{metric}_ratio_to_gbfs"] = (
                    sum(row[metric] for row in combined.values()) / base_total if base_total else None
                )
            item["estimated_fallback"] = estimated
            item["additional_found_case_repeats"] = [
                list(key) for key in sorted(runs)
                if runs[key]["result"] == "OK" and reference[key]["result"] != "OK"
            ]
            item["missed_baseline_found_case_repeats"] = [
                list(key) for key in sorted(runs)
                if runs[key]["result"] != "OK" and reference[key]["result"] == "OK"
            ]
        variants.append(item)
    return {
        "schema": "learned-reverse-benchmark-report-v1",
        "csv_path": str(path),
        "csv_sha256": digest(path),
        "cases": len({case for case, _ in reference}),
        "repeats": len({repeat for _, repeat in reference}),
        "baseline": asdict(baseline),
        "variants": variants,
        "notes": [
            "All direct times include ranking-model inference and internal witness verification.",
            "External independent witness verification is outside the search timers.",
            "Model inference time is a component of search time, not an extra cost to add.",
            "Estimated fallback is calculated from paired rows; actual pipeline times are measured separately.",
            "GBFS max_nodes counts registrations; learned search max_nodes counts generated candidates.",
        ],
    }, grouped, baseline


def select_model(result, grouped, baseline, validation_seed):
    reference = grouped[baseline]
    if any(str(row["seed"]) != str(validation_seed) for row in reference.values()):
        raise ValueError("model selection is allowed only on the declared validation seed")
    if result["cases"] != 60 or result["repeats"] != 3:
        raise ValueError("the prospective search validation requires 60 cases and 3 repeats")
    if {row["stones"] for row in reference.values()} != {16, 24, 32, 40, 48, 56}:
        raise ValueError("the prospective validation stone counts do not match")
    if Counter(row["stones"] for (case, repeat), row in reference.items() if repeat == 1) != {
        stones: 10 for stones in (16, 24, 32, 40, 48, 56)
    }:
        raise ValueError("the prospective validation requires 10 goals per stone count")
    if baseline.threads != 1 or baseline.max_nodes != 100000:
        raise ValueError("the prospective GBFS settings do not match")
    if any(row["forward_discs"] != 8 or row["known_reachable"] != "true" for row in reference.values()):
        raise ValueError("the prospective validation requires known-reachable goals and an 8-stone boundary")
    candidates = [row for row in result["variants"] if row["variant"]["algorithm"] == "learned-beam"]
    if sorted(row["model_hidden_size"] for row in candidates) != [0, 16]:
        raise ValueError("selection requires exactly the planned linear and width-16 MLP models")
    for row in candidates:
        config = row["variant"]
        if config["width"] != 32 or config["max_nodes"] != 100000 or config["threads"] != 1:
            raise ValueError("model selection settings differ from the prospective plan")
        if not row["estimated_fallback"]["outcomes_and_search_counts_identical_across_repeats"]:
            raise ValueError("selection expected deterministic outcomes and counts")
        model_path = Path(row["model_path"])
        if digest(model_path) != config["model_sha256"]:
            raise ValueError("a model file changed after the measured validation")
    best_ok = max(row["estimated_fallback"]["per_repeat"][0]["ok"] for row in candidates)
    tied = [row for row in candidates if row["estimated_fallback"]["per_repeat"][0]["ok"] == best_ok]
    best = min(tied, key=lambda row: row["estimated_fallback"]["median_search_seconds"])
    chosen = best
    for row in tied:
        if row["model_hidden_size"] == 0 and row["estimated_fallback"]["median_search_seconds"] <= (
            1.05 * best["estimated_fallback"]["median_search_seconds"]
        ):
            chosen = row
    return {
        "schema": "learned-ranker-selection-v1",
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "validation_csv_sha256": result["csv_sha256"],
        "validation_seed": validation_seed,
        "selected_model": chosen["model_path"],
        "selected_model_sha256": chosen["variant"]["model_sha256"],
        "selected_hidden_size": chosen["model_hidden_size"],
        "settings": {"forward_discs": 8, "width": 32, "beam_generated_limit": 100000,
                     "gbfs_registered_limit": 100000, "threads": 1},
        "rule": "maximize fallback Found; minimize median estimated fallback time; prefer linear within 5 percent",
        "candidates": [{
            "model": row["model_path"],
            "sha256": row["variant"]["model_sha256"],
            "hidden_size": row["model_hidden_size"],
            "fallback_found": row["estimated_fallback"]["per_repeat"][0]["ok"],
            "median_estimated_fallback_seconds": row["estimated_fallback"]["median_search_seconds"],
        } for row in candidates],
        "limitations": "Selection times sum independent searches. Actual pipeline measurements follow only after freezing settings.",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--selection-out", type=Path)
    parser.add_argument("--validation-seed", type=int, default=20261126)
    args = parser.parse_args()
    result, grouped, baseline = report(args.csv)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    if args.selection_out:
        selection = select_model(result, grouped, baseline, args.validation_seed)
        args.selection_out.write_text(json.dumps(selection, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(selection, indent=2))


if __name__ == "__main__":
    main()
