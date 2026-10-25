#!/usr/bin/env python3
"""Capture the fixed learned-search experiment outside the repository.

Each subprocess finishes before its outputs are checked. Every OK is matched to
a witness and independently replayed; only then are complete files published
with atomic per-file replacement and byte-for-byte hash checks. This script
does not change models, search settings, or algorithm implementations.
"""

import argparse
import csv
import hashlib
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict
from datetime import datetime, timezone
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def run(command, directory, stem):
    log = directory / f"{stem}.log"
    print(f"START {stem}", flush=True)
    with log.open("w", encoding="utf-8") as stream:
        process = subprocess.Popen(command, cwd=REPO, stdout=stream, stderr=subprocess.STDOUT)
        while True:
            try:
                status = process.wait(timeout=30)
                break
            except subprocess.TimeoutExpired:
                lines = log.read_text(encoding="utf-8").splitlines()
                print(f"PROGRESS {stem}: {lines[-1] if lines else 'running'}", flush=True)
    if status:
        raise RuntimeError(f"{stem} exited {status}; log retained at {log}")


def publish(paths, output, capture):
    output.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix="learned-capture-publish-", dir=output.parent))
    manifest = {}
    try:
        for source in paths:
            staged = staging / source.name
            shutil.copy2(source, staged)
            expected = digest(source)
            if digest(staged) != expected:
                raise RuntimeError(f"staging changed bytes: {source}")
            manifest[source.name] = expected
        for source in paths:
            os.replace(staging / source.name, output / source.name)
        for name, expected in manifest.items():
            if digest(output / name) != expected:
                raise RuntimeError(f"publication changed bytes: {name}")
        write_json(output / f"{capture}_capture.json", {
            "schema": "verified-learned-benchmark-capture-v1",
            "completed_at_utc": datetime.now(timezone.utc).isoformat(),
            "driver_sha256": digest(Path(__file__).resolve()),
            "independent_verifier_sha256": digest(REPO / "scripts/verify_records_independent.py"),
            "all_files_closed_before_publication": True,
            "publication": "atomic replacement per file, followed by SHA-256 verification; marker written last",
            "files": manifest,
        })
    finally:
        shutil.rmtree(staging)


def verify_records(records, positions, report, directory, benchmark=None):
    command = [sys.executable, str(REPO / "scripts/verify_records_independent.py"),
               str(records), "--positions", str(positions), "--output", str(report)]
    if benchmark:
        command += ["--benchmark-csv", str(benchmark)]
    run(command, directory, report.stem)


def direct(args, name, root):
    directory = root / name
    directory.mkdir()
    files = {suffix: directory / f"{name}_{suffix}" for suffix in (
        "positions.txt", "records.csv", "exclusions.json", "independent_verification.json",
        "source_records.csv", "source_independent_verification.json", "report.json",
    )}
    results = directory / f"{name}.csv"
    command = [str(REPO / "target/release/bench_reverse_search"), "--discs", "8",
               "--max-nodes", "100000", "--threads", "1", "--widths", "32",
               "--repeats", "3", "--algorithms",
               "gbfs,best-first-mobility,learned-beam,learned-best-first",
               "--model", "research/learned_beam/models/linear.json",
               "--model", "research/learned_beam/models/mlp16.json",
               "--output", str(results), "--positions-out", str(files["positions.txt"]),
               "--records-out", str(files["records.csv"]),
               "--exclusion-report-out", str(files["exclusions.json"])]
    for path in args.exclude_keys:
        command += ["--exclude-keys", str(path)]
    if name == "holdout":
        command += ["--seed", "20261127", "--samples", "24", "--stones",
                    "16,24,32,40,48,56", "--source-records-out", str(files["source_records.csv"])]
    else:
        source = (REPO / "research/beam_search/legacy_sat_positions.txt" if name == "legacy_sat"
                  else REPO / "research/learned_beam/evaluation_sat/sat_exact_UNKNOWN.txt")
        command += ["--input", str(source)]
    run(command, directory, f"{name}_benchmark")
    with results.open(newline="") as stream:
        expected = Counter(int(row["case"]) for row in csv.DictReader(stream) if row["result"] == "OK")
    with files["records.csv"].open(newline="") as stream:
        actual = Counter(int(row[0]) for row in csv.reader(stream))
    if expected != actual:
        raise RuntimeError(f"{name}: benchmark OK/witness coverage mismatch")
    verify_records(files["records.csv"], files["positions.txt"],
                   files["independent_verification.json"], directory, results)
    if name == "holdout":
        verify_records(files["source_records.csv"], files["positions.txt"],
                       files["source_independent_verification.json"], directory)
    run([sys.executable, str(REPO / "scripts/report_learned_benchmark.py"), str(results),
         "--output", str(files["report.json"])], directory, f"{name}_summary")
    published = [results] + [path for path in files.values() if path.exists()]
    publish(published, args.output_dir, name)
    report = json.loads(files["report.json"].read_text())
    table = [{"algorithm": row["variant"]["algorithm"], "hidden": row["model_hidden_size"],
              "ok": row["direct"]["per_repeat"][0]["ok"],
              "generated": row["direct"]["per_repeat"][0]["generated"],
              "median_seconds": row["direct"]["median_search_seconds"]}
             for row in report["variants"]]
    print(json.dumps({"completed": name, "verified_witnesses": sum(actual.values()),
                      "results": table, "temporary_directory": str(directory)}), flush=True)
    return files["positions.txt"]


def pipeline(args, name, positions, root):
    directory = root / f"{name}_pipeline"
    directory.mkdir()
    work = directory / "trials"
    results = directory / f"{name}_pipeline.csv"
    run([sys.executable, str(REPO / "scripts/bench_reverse_pipeline.py"), "--input", str(positions),
         "--binary-dir", str(REPO / "target/release"), "--work-dir", str(work),
         "--output", str(results), "--repeats", "3", "--discs", "8",
         "--max-nodes", "100000", "--beam-width", "32", "--threads", "1",
         "--model", str(REPO / "research/learned_beam/models/linear.json")],
        directory, f"{name}_pipeline_measurement")
    original = positions.read_text().splitlines()
    first_index = {}
    for index, position in enumerate(original, 1):
        first_index.setdefault(position, index)
    combined = directory / f"{name}_pipeline_records.csv"
    manifests = []
    with combined.open("w", newline="") as output:
        writer = csv.writer(output, lineterminator="\n")
        for source in sorted(work.rglob("reverse_exact_records.csv")):
            stage, trial = source.parent.name, source.parent.parent
            inputs = positions if stage == "beam" or "baseline" in trial.name else (
                trial / "beam/reverse_exact_UNKNOWN.txt")
            proof_report = source.parent / "independent_verification.json"
            verify_records(source, inputs, proof_report, directory)
            proof = json.loads(proof_report.read_text())
            manifests.append({"stage": str(source.relative_to(work)), **proof})
            with source.open(newline="") as stream:
                for fields in csv.reader(stream):
                    fields[0] = str(first_index[fields[2]])
                    writer.writerow(fields)
    with results.open(newline="") as stream:
        rows = list(csv.DictReader(stream))
    with combined.open(newline="") as stream:
        witness_count = sum(1 for _ in csv.reader(stream))
    if witness_count != sum(int(row["ok"]) for row in rows):
        raise RuntimeError("pipeline proof count differs from the sum of both methods' OK counts")
    verification = directory / f"{name}_pipeline_independent_verification.json"
    verify_records(combined, positions, verification, directory)
    groups = defaultdict(list)
    for row in rows:
        groups[row["method"]].append(row)
    summary = {
        "schema": "verified-learned-pipeline-report-v1",
        "time_kind": "actual command wall time including model loading, forward-table construction and I/O",
        "certificates": witness_count,
        "combined_certificate_indices": "reindexed by exact goal into the original corpus; actions unchanged",
        "stages": manifests,
        "methods": {method: {
            "ok_per_repeat": [int(row["ok"]) for row in values],
            "ng_per_repeat": [int(row["ng"]) for row in values],
            "unknown_per_repeat": [int(row["unknown"]) for row in values],
            "total_seconds": [float(row["total_seconds"]) for row in values],
            "median_total_seconds": statistics.median(float(row["total_seconds"]) for row in values),
            "mean_total_seconds": statistics.mean(float(row["total_seconds"]) for row in values),
        } for method, values in groups.items()},
    }
    report = directory / f"{name}_pipeline_report.json"
    write_json(report, summary)
    publish([results, combined, verification, report], args.output_dir, f"{name}_pipeline")
    print(json.dumps({"completed": f"{name}_pipeline", "certificates": witness_count,
                      "methods": summary["methods"]}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exclude-keys", type=Path, action="append", required=True)
    parser.add_argument("--output-dir", type=Path, default=REPO / "research/learned_beam")
    parser.add_argument("--dataset", choices=("all", "holdout", "legacy_sat", "sampled_sat"), default="all")
    parser.add_argument("--skip-pipeline", action="store_true")
    args = parser.parse_args()
    args.exclude_keys = [path.resolve(strict=True) for path in args.exclude_keys]
    args.output_dir = args.output_dir.resolve()
    root = Path(tempfile.mkdtemp(prefix="othello-learned-evaluation-"))
    print(f"ISOLATED_OUTPUT {root}", flush=True)
    names = ["holdout", "legacy_sat", "sampled_sat"] if args.dataset == "all" else [args.dataset]
    sources = {name: direct(args, name, root) for name in names}
    if not args.skip_pipeline:
        for name in names:
            pipeline(args, name, sources[name], root)
    print(f"COMPLETE {root}", flush=True)


if __name__ == "__main__":
    main()
