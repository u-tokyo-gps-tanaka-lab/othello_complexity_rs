#!/usr/bin/env python3
"""Measure GBFS and an actual beam-to-GBFS command-line pipeline.

Unlike estimated_fallback in summarize_reverse_bench.py, this executes the
second stage on the beam's UNKNOWN file. Timings include process startup,
forward-table construction, search, and command I/O; verification is untimed.
"""

import argparse
import csv
import hashlib
import json
import re
import subprocess
import sys
import tempfile
import time
from collections import Counter
from dataclasses import dataclass
from pathlib import Path


POSITION = re.compile(r"[XO-]{64} [bw]")
NODE_LINE = re.compile(r"input ([0-9]+): (OK|NG|UNKNOWN) nodes=([0-9]+)")
STATUSES = ("OK", "NG", "UNKNOWN")
INITIAL = "---------------------------OX------XO--------------------------- b"
FIELDS = (
    "repeat", "method", "cases", "ok", "ng", "unknown", "beam_seconds",
    "gbfs_seconds", "total_seconds", "beam_generated", "gbfs_registered",
    "beam_algorithm", "beam_max_nodes", "gbfs_max_nodes", "model_sha256",
)


@dataclass
class Stage:
    directory: Path
    classifications: dict
    seconds: float
    nodes: int


def read_positions(path):
    positions = path.read_text(encoding="utf-8").splitlines()
    for number, position in enumerate(positions, 1):
        if not POSITION.fullmatch(position):
            raise ValueError(f"{path}:{number}: expected a 66-character exact position")
        if any(position[square] == "-" for square in (27, 28, 35, 36)):
            raise ValueError(f"{path}:{number}: the central four squares must be occupied")
    return positions


def check_partition(classifications, expected, label):
    combined = sum(classifications.values(), Counter())
    if combined != Counter(expected):
        raise ValueError(f"{label}: OK/NG/UNKNOWN do not partition the input with multiplicity")
    if classifications["OK"].keys() & classifications["NG"].keys():
        raise ValueError(f"{label}: the same exact position was classified both OK and NG")


def run_command(command, directory, stem):
    stdout_path = directory / f"{stem}.stdout.log"
    stderr_path = directory / f"{stem}.stderr.log"
    with stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open(
        "w", encoding="utf-8"
    ) as stderr:
        started = time.perf_counter()
        completed = subprocess.run(command, stdout=stdout, stderr=stderr, check=False)
        seconds = time.perf_counter() - started
    if completed.returncode:
        raise RuntimeError(
            f"{stem} exited with status {completed.returncode}; see {stderr_path}"
        )
    return seconds, stdout_path


def parse_node_counts(stdout_path, inputs, classifications):
    rows = []
    for line in stdout_path.read_text(encoding="utf-8").splitlines():
        if match := NODE_LINE.fullmatch(line):
            index, status, nodes = match.groups()
            rows.append((int(index), status, int(nodes)))
    if [index for index, _, _ in rows] != list(range(1, len(inputs) + 1)):
        raise ValueError(f"{stdout_path}: missing or repeated per-input node counts")
    reported = {status: Counter() for status in STATUSES}
    for index, status, _ in rows:
        reported[status][inputs[index - 1]] += 1
    if reported != classifications:
        raise ValueError(f"{stdout_path}: classifications disagree with output files")
    return sum(nodes for _, _, nodes in rows)


def verify_records(args, trial, name, output, inputs, classifications):
    path = output / "reverse_exact_records.csv"
    goals = Counter()
    indices = set()
    with path.open(newline="", encoding="utf-8") as source:
        for fields in csv.reader(source, strict=True):
            if len(fields) != 6:
                raise ValueError(f"{path}: expected six record fields")
            index = int(fields[0])
            if index in indices or not 1 <= index <= len(inputs):
                raise ValueError(f"{path}: repeated or invalid input index {index}")
            if fields[1] != INITIAL or fields[2] != inputs[index - 1] or fields[3] != "exact":
                raise ValueError(f"{path}: certificate does not describe its exact input")
            indices.add(index)
            goals[fields[2]] += 1
    if goals != classifications["OK"]:
        raise ValueError(f"{path}: certificates do not match the OK output with multiplicity")
    # No verification time contributes to the measured search command times.
    run_command(
        [str(args.binary_dir / "verify_record"), "--input-file", str(path)],
        trial,
        f"{name}.verify",
    )


def run_stage(args, trial, name, algorithm, source):
    inputs = read_positions(source)
    output = trial / name
    command = [
        str(args.binary_dir / "reverse_search"), algorithm, str(source),
        "--mode", "exact", "--discs", str(args.discs),
        "--max-nodes", str(args.beam_max_nodes if algorithm in ("beam", "learned-beam")
                           else args.max_nodes), "--out-dir", str(output),
    ]
    if algorithm == "beam":
        command.extend([
            "--beam-width", str(args.beam_width), "--heuristic", args.heuristic,
        ])
    elif algorithm == "learned-beam":
        command.extend([
            "--beam-width", str(args.beam_width), "--model", str(args.model),
        ])
    else:
        command.extend(["--threads", str(args.threads)])
    seconds, stdout_path = run_command(command, trial, name)
    classifications = {
        status: Counter(read_positions(output / f"reverse_exact_{status}.txt"))
        for status in STATUSES
    }
    check_partition(classifications, inputs, output)
    if algorithm == "learned-beam":
        if classifications["NG"]:
            raise ValueError("learned witness search returned NG")
        metadata = json.loads((output / "learned_model_metadata.json").read_text(encoding="utf-8"))
        if metadata["model_sha256"] != args.model_sha256:
            raise ValueError("the model changed between benchmark setup and search")
        if metadata["verdict_policy"] != "verified-witness-or-unknown":
            raise ValueError("unexpected learned-search verdict policy")
    nodes = parse_node_counts(stdout_path, inputs, classifications)
    verify_records(args, trial, name, output, inputs, classifications)
    return Stage(output, classifications, seconds, nodes)


def run_trial(args, repeat, method, inputs):
    # Unique directories preserve earlier runs and unrelated work-dir files.
    trial = Path(tempfile.mkdtemp(prefix=f"repeat-{repeat:03d}-{method}-", dir=args.work_dir))
    beam_seconds = gbfs_seconds = 0.0
    beam_generated = gbfs_registered = 0
    if method == "baseline":
        gbfs = run_stage(args, trial, "gbfs", "gbfs-parallel", args.input)
        classifications = gbfs.classifications
        gbfs_seconds, gbfs_registered = gbfs.seconds, gbfs.nodes
    else:
        beam = run_stage(args, trial, "beam", "learned-beam" if args.model else "beam", args.input)
        beam_seconds, beam_generated = beam.seconds, beam.nodes
        classifications = {status: counts.copy() for status, counts in beam.classifications.items()}
        if classifications["UNKNOWN"]:
            gbfs = run_stage(
                args, trial, "gbfs", "gbfs-parallel",
                beam.directory / "reverse_exact_UNKNOWN.txt",
            )
            gbfs_seconds, gbfs_registered = gbfs.seconds, gbfs.nodes
            classifications["OK"] += gbfs.classifications["OK"]
            classifications["NG"] += gbfs.classifications["NG"]
            classifications["UNKNOWN"] = gbfs.classifications["UNKNOWN"]
    check_partition(classifications, inputs, trial)
    row = {
        "repeat": repeat,
        "method": method,
        "cases": len(inputs),
        "ok": sum(classifications["OK"].values()),
        "ng": sum(classifications["NG"].values()),
        "unknown": sum(classifications["UNKNOWN"].values()),
        "beam_seconds": f"{beam_seconds:.9f}",
        "gbfs_seconds": f"{gbfs_seconds:.9f}",
        "total_seconds": f"{beam_seconds + gbfs_seconds:.9f}",
        "beam_generated": beam_generated,
        "gbfs_registered": gbfs_registered,
        "beam_algorithm": ("learned-beam" if args.model else "beam") if method != "baseline" else "",
        "beam_max_nodes": args.beam_max_nodes if method != "baseline" else "",
        "gbfs_max_nodes": args.max_nodes,
        "model_sha256": args.model_sha256 if method != "baseline" else "",
    }
    print(
        f"repeat {repeat} {method}: OK={row['ok']} NG={row['ng']} "
        f"UNKNOWN={row['unknown']} {row['total_seconds']} s; logs: {trial}",
        file=sys.stderr,
        flush=True,
    )
    return row, classifications


def compare_methods(baseline, pipeline):
    for status in ("OK", "NG"):
        if baseline[status] - pipeline[status]:
            raise ValueError(f"two-stage did not preserve all baseline {status} results")
    if (baseline["OK"].keys() & pipeline["NG"].keys()
            or baseline["NG"].keys() & pipeline["OK"].keys()):
        raise ValueError("baseline and two-stage disagree between OK and NG")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--binary-dir", type=Path, default=Path("target/release"))
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--discs", type=int, default=8)
    parser.add_argument("--max-nodes", type=int, default=100_000)
    parser.add_argument("--beam-max-nodes", type=int, help="Override the first stage's generated-candidate budget")
    parser.add_argument("--beam-width", type=int, default=32)
    ranker = parser.add_mutually_exclusive_group()
    ranker.add_argument("--heuristic", choices=("branching", "mobility", "coherence", "frontier", "continuation"), default="mobility")
    ranker.add_argument("--model", type=Path, help="Use learned-beam with this model instead of the hand-written heuristic")
    parser.add_argument("--threads", type=int, default=1)
    args = parser.parse_args()
    args.beam_max_nodes = args.max_nodes if args.beam_max_nodes is None else args.beam_max_nodes
    if min(args.repeats, args.beam_width, args.threads) <= 0 or min(args.max_nodes, args.beam_max_nodes) < 0:
        parser.error("repeats, beam-width, and threads must be positive; max-nodes must be nonnegative")
    if not 4 <= args.discs <= 64:
        parser.error("discs must be between 4 and 64")
    args.input = args.input.resolve(strict=True)
    args.binary_dir = args.binary_dir.resolve(strict=True)
    args.work_dir = args.work_dir.resolve()
    args.output = args.output.resolve()
    args.model_sha256 = ""
    if args.model:
        args.model = args.model.resolve(strict=True)
        args.model_sha256 = hashlib.sha256(args.model.read_bytes()).hexdigest()
    binaries = [args.binary_dir / name for name in ("reverse_search", "verify_record")]
    if any(not binary.is_file() for binary in binaries):
        parser.error("binary-dir must contain reverse_search and verify_record")
    if args.output in [args.input, args.model, *binaries]:
        parser.error("output must not overwrite the input or an executable")
    inputs = read_positions(args.input)
    args.work_dir.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    known_ok, known_ng = set(), set()
    with args.output.open("w", newline="", encoding="utf-8") as output:
        writer = csv.DictWriter(output, fieldnames=FIELDS, lineterminator="\n")
        writer.writeheader()
        output.flush()
        for repeat in range(1, args.repeats + 1):
            methods = ("baseline", "two-stage") if repeat % 2 else ("two-stage", "baseline")
            results = {}
            for method in methods:
                row, classifications = run_trial(args, repeat, method, inputs)
                if (known_ok & classifications["NG"].keys()
                        or known_ng & classifications["OK"].keys()):
                    raise ValueError("trials disagree between OK and NG")
                known_ok.update(classifications["OK"])
                known_ng.update(classifications["NG"])
                results[method] = classifications
                if len(results) == 2:
                    compare_methods(results["baseline"], results["two-stage"])
                writer.writerow(row)
                output.flush()


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, csv.Error) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
