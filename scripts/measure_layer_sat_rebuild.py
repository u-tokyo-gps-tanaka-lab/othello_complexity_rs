#!/usr/bin/env python3
"""Reproduce the fixed-query layer_sat comparison recorded under docs/measurements.

The baseline source is archived into a temporary directory and changed only to
measure encoding and solver calls. The baseline worktree remains unchanged.
"""

import argparse
import difflib
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import time
from datetime import datetime, timezone


SYMMETRIES = (
    "identity", "rotate90", "rotate180", "rotate270", "mirror_horizontal",
    "mirror_vertical", "transpose", "anti_transpose",
)
SIDE_NAMES = {"b": "black", "w": "white"}
CATEGORIES = ("OK", "NG", "UNKNOWN")


def command(argv, cwd=None):
    return subprocess.run(argv, cwd=cwd, check=True, text=True, capture_output=True)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")


def replace_once(text, before, after):
    if text.count(before) != 1:
        raise ValueError(f"baseline instrumentation anchor is not unique: {before!r}")
    return text.replace(before, after, 1)


def instrument_baseline(source):
    text = replace_once(source, "use std::time::Duration;", "use std::time::{Duration, Instant};")
    text = replace_once(
        text,
        "        let vars = SatVars::new(max_plies, rays, flip_sources);",
        "        let build_started = Instant::now();\n"
        "        let vars = SatVars::new(max_plies, rays, flip_sources);",
    )
    text = replace_once(
        text,
        "        dump_dimacs_cnf(\n",
        "        let build_seconds = build_started.elapsed().as_secs_f64();\n"
        "        if verbose {\n"
        '            println!("[measurement_encoding] vars={} clauses={} build_secs={:.6}",\n'
        "                vars.var_count, clauses.len(), build_seconds);\n"
        "        }\n"
        "        dump_dimacs_cnf(\n",
    )
    return replace_once(
        text,
        "        match solve_single_problem(&vars, &clauses, sat_timeout_per_instance)? {",
        "        let solve_started = Instant::now();\n"
        "        let measured_result = solve_single_problem(&vars, &clauses, sat_timeout_per_instance)?;\n"
        "        if verbose {\n"
        '            println!("[measurement_solver] solve_secs={:.6}",\n'
        "                solve_started.elapsed().as_secs_f64());\n"
        "        }\n"
        "        match measured_result {",
    )


def prepare(args):
    baseline = args.baseline.resolve()
    scratch = args.scratch.resolve()
    artifacts = args.artifacts.resolve()
    scratch.mkdir(parents=True, exist_ok=False)
    artifacts.mkdir(parents=True, exist_ok=False)
    if command(["git", "status", "--porcelain", "--untracked-files=no"], baseline).stdout:
        raise ValueError("baseline worktree must be clean")
    commit = command(["git", "rev-parse", "HEAD"], baseline).stdout.strip()
    archive = subprocess.run(
        ["git", "archive", "--format=tar", commit], cwd=baseline,
        check=True, capture_output=True,
    ).stdout
    source_root = scratch / "baseline-source"
    source_root.mkdir()
    with tarfile.open(fileobj=io.BytesIO(archive)) as archive_file:
        archive_file.extractall(source_root, filter="data")
    relative_core = "src/prunings/layer_sat.rs"
    core = source_root / relative_core
    before = core.read_text()
    after = instrument_baseline(before)
    core.write_text(after)
    patch = "".join(difflib.unified_diff(
        before.splitlines(keepends=True), after.splitlines(keepends=True),
        fromfile=f"a/{relative_core}", tofile=f"b/{relative_core}", n=0,
    ))
    (artifacts / "baseline-timing.patch").write_text(patch)
    cached_binary = baseline / "target/release/layer_sat"
    original_binary = scratch / "baseline-uninstrumented"
    shutil.copy2(cached_binary, original_binary)
    build_command = [
        "cargo", "build", "--release", "--locked", "--bin", "layer_sat", "-j", "4",
        "--target-dir", str(baseline / "target"),
    ]
    try:
        build = command(build_command, source_root)
        measured_binary = scratch / "baseline-layer-sat"
        shutil.copy2(cached_binary, measured_binary)
    finally:
        shutil.copy2(original_binary, cached_binary)
    if command(["git", "status", "--porcelain", "--untracked-files=no"], baseline).stdout:
        raise ValueError("baseline source changed during preparation")
    write_json(artifacts / "baseline.json", {
        "commit": commit,
        "source_root": str(baseline),
        "instrumented_source_root": str(source_root),
        "binary": str(measured_binary),
        "binary_sha256": sha256(measured_binary),
        "uninstrumented_binary_sha256": sha256(original_binary),
        "patch_sha256": sha256(artifacts / "baseline-timing.patch"),
        "build_command": build_command,
        "build_stdout": build.stdout,
        "build_stderr": build.stderr,
        "allocator": "mimalloc 0.1.52",
        "core_sha256": sha256(core),
    })
    print(f"Prepared baseline {commit} at {measured_binary}", flush=True)


def position(board, side):
    if len(board) != 64 or set(board) - set("XO-") or side not in SIDE_NAMES:
        raise ValueError("invalid measurement position")
    return f"{board} {side}"


def transform(state, symmetry):
    board, side = state.split()
    transformed = ["-"] * 64
    number = SYMMETRIES.index(symmetry)
    for square, cell in enumerate(board):
        x, y = square % 8, square // 8
        u, v = (
            (x, y), (7 - y, x), (7 - x, 7 - y), (y, 7 - x),
            (7 - x, y), (x, 7 - y), (y, x), (7 - y, 7 - x),
        )[number]
        transformed[v * 8 + u] = cell
    return position("".join(transformed), side)


def captures(board, square):
    if board[square] != "-":
        return []
    result = []
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            if (dx, dy) == (0, 0):
                continue
            x, y = square % 8 + dx, square // 8 + dy
            line = []
            while 0 <= x < 8 and 0 <= y < 8 and board[y * 8 + x] == "O":
                line.append(y * 8 + x)
                x, y = x + dx, y + dy
            if line and 0 <= x < 8 and 0 <= y < 8 and board[y * 8 + x] == "X":
                result.extend(line)
    return result


def moves(board):
    return [square for square in range(64) if captures(board, square)]


def apply(state, action):
    board, side = state.split()
    swapped = board.translate(str.maketrans("XO", "OX"))
    if action == "ps":
        if moves(board) or not moves(swapped):
            raise ValueError("illegal pass in measured witness")
        return position(swapped, "w" if side == "b" else "b")
    if re.fullmatch(r"[A-H][1-8]", action) is None:
        raise ValueError(f"invalid measured action: {action}")
    square = (int(action[1]) - 1) * 8 + ord(action[0]) - ord("A")
    flipped = captures(board, square)
    if not flipped:
        raise ValueError("illegal placement in measured witness")
    updated = list(board)
    for square in [square] + flipped:
        updated[square] = "X"
    return position(
        "".join(updated).translate(str.maketrans("XO", "OX")),
        "w" if side == "b" else "b",
    )


def replay(state, actions):
    for action in actions:
        state = apply(state, action)
    return state


def enumerate_reachable(start, cap):
    seen, stack = set(), [start]
    while stack:
        state = stack.pop()
        if state in seen:
            continue
        seen.add(state)
        board, side = state.split()
        legal = moves(board)
        if not legal:
            swapped = board.translate(str.maketrans("XO", "OX"))
            if moves(swapped):
                stack.append(position(swapped, "w" if side == "b" else "b"))
        elif 64 - board.count("-") < cap:
            for square in legal:
                action = f"{chr(ord('A') + square % 8)}{square // 8 + 1}"
                stack.append(apply(state, action))
    return seen


def representatives(start, goal, mode):
    if mode == "exact":
        return [("identity", goal)]
    stabilizer = [sym for sym in SYMMETRIES if transform(start, sym) == start]
    seen, candidates = set(), []
    for symmetry in SYMMETRIES:
        image = transform(goal, symmetry)
        canonical = min(transform(image, fixed) for fixed in stabilizer)
        if canonical not in seen:
            seen.add(canonical)
            candidates.append((symmetry, image))
    return candidates


def make_inputs(root):
    initial = position("---------------------------OX------XO---------------------------", "b")
    one = apply(initial, "D3")
    three = replay(initial, ["D3", "C3", "C4"])
    forced = position("OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO", "w")
    before_pass = apply(forced, "H2")
    after_pass = apply(before_pass, "ps")
    terminal = apply(after_pass, "H7")
    single = position("OXX-OOOOOXXXXXXOOXOOOXXOOOXOXOXXOXOXOOXXOOOXXXOXOOXOOOXXOOOOOOXX", "w")
    full = position("X" * 64, "b")
    fixed = [
        ("initial_zero", initial, initial),
        ("initial_other_turn", initial, initial[:-1] + "w"),
        ("initial_reflected", initial, transform(initial, "mirror_horizontal")),
        ("opening", initial, one),
        ("opening_other_turn", initial, one[:-1] + "b"),
        ("three_placements", initial, three),
        ("three_other_turn", initial, three[:-1] + "b"),
        ("decreasing_discs", one, initial),
        ("pass_only", single, apply(single, "ps")),
        ("pass_other_turn", single, apply(single, "ps")[:-1] + "w"),
        ("leading_pass", before_pass, terminal),
        ("middle_pass", forced, terminal),
        ("trailing_pass", forced, after_pass),
        ("terminal_zero", full, full),
        ("terminal_pass", full, position("O" * 64, "w")),
    ]
    cases = []
    for name, start, goal in fixed:
        cap = max(64 - start[:64].count("-"), 64 - goal[:64].count("-"))
        reachable = enumerate_reachable(start, cap)
        cases.append({"name": name, "start": start, "goal": goal, "mode": "exact",
                      "oracle": "OK" if goal in reachable else "NG"})
        if name in {"initial_reflected", "initial_other_turn", "three_placements", "three_other_turn", "pass_only"}:
            orbit_ok = any(transform(goal, sym) in reachable for sym in SYMMETRIES)
            cases.append({"name": name + "_symmetry", "start": start, "goal": goal,
                          "mode": "symmetry", "oracle": "OK" if orbit_ok else "NG"})
    research_path = root / "research/pass24.txt"
    research_lines = research_path.read_text().splitlines()
    if len(research_lines) != 1:
        raise ValueError("measurement requires the specified single research/pass24.txt row")
    for side in ("b", "w"):
        for mode in ("exact", "symmetry"):
            cases.append({"name": f"research_pass24_{side}_{mode}", "start": initial,
                          "goal": position(research_lines[0], side), "mode": mode,
                          "oracle": None})
    for case in cases:
        case["candidates"] = representatives(case["start"], case["goal"], case["mode"])
    return cases, {"path": "research/pass24.txt", "sha256": sha256(research_path),
                   "content": research_path.read_text()}


def execute(binary, start, goal, mode, implementation, output, timeout):
    output.mkdir(parents=True, exist_ok=False)
    args = [str(binary)]
    if implementation == "new":
        args += ["--start", start, "--goal", goal]
        if mode == "symmetry":
            args += ["--check-symmetry"]
        prefix = f"layer_sat_{mode}"
    else:
        start_board, start_side = start.split()
        goal_board, goal_side = goal.split()
        args += ["--start", start_board, "--start-turn", SIDE_NAMES[start_side],
                 "--goal", goal_board, "--goal-side-to-move", SIDE_NAMES[goal_side]]
        prefix = "layer_sat"
    args += ["--out-dir", str(output), "--parallel-goals", "1",
             "--sat-timeout-secs", str(timeout), "--verbose"]
    timing = output / "time.json"
    argv = ["/usr/bin/time", "-f",
            '{"wall_seconds":%e,"user_seconds":%U,"system_seconds":%S,"max_rss_kib":%M}',
            "-o", str(timing)] + args
    started = time.monotonic()
    process = command(argv)
    elapsed = time.monotonic() - started
    categories = {category: (output / f"{prefix}_{category}.txt").read_text().splitlines()
                  for category in CATEGORIES}
    if sum(map(len, categories.values())) != 1:
        raise ValueError("single measurement query did not produce exactly one classification")
    classification = next(category for category, lines in categories.items() if lines)
    encodings = []
    for line in process.stdout.splitlines():
        if line.startswith(("[encoding]", "[measurement_encoding]")):
            match = re.search(r"vars=(\d+) clauses=(\d+) build_secs=([0-9.]+)", line)
            if match is None:
                raise ValueError(f"unrecognized encoding metrics: {line}")
            metric = {"variables": int(match[1]), "clauses": int(match[2]),
                      "build_seconds": float(match[3])}
            if implementation == "new":
                metric["goal"] = line.split("goal=", 1)[1].split(" range=", 1)[0]
            else:
                metric["goal"] = goal
            encodings.append(metric)
    solve_times = [float(value) for value in re.findall(r"solve_secs=([0-9.]+)", process.stdout)]
    if len(solve_times) != len(encodings):
        raise ValueError("encoding and solver metric counts differ")
    for metric, seconds in zip(encodings, solve_times):
        metric["solve_seconds"] = seconds
    record_text = None
    if classification == "OK":
        if implementation == "new":
            record_text = (output / f"{prefix}_records.tsv").read_text()
            rows = record_text.splitlines()
            if len(rows) != 1:
                raise ValueError("SAT measurement must have exactly one record")
            index, record_start, record_goal, record_mode, symmetry, actions = rows[0].split("\t")
            if (index, record_start, record_goal, record_mode) != ("1", start, goal, mode):
                raise ValueError("SAT record does not preserve measured query")
            if mode == "exact" and symmetry != "identity":
                raise ValueError("exact SAT record selected a nonidentity transform")
            endpoint = replay(start, actions.split())
            if endpoint != transform(goal, symmetry):
                raise ValueError("independent replay rejected the new SAT record")
        else:
            records = re.findall(r"^\[1\] Record: (.*)$", process.stdout, re.MULTILINE)
            if len(records) != 1 or len(records[0]) % 2:
                raise ValueError("baseline SAT witness is missing or malformed")
            record_text = records[0]
            if replay(start, [record_text[n:n + 2] for n in range(0, len(record_text), 2)]) != goal:
                raise ValueError("independent replay rejected the baseline SAT witness")
    return {"command": argv, "classification": classification, "classifications": categories,
            "record": record_text, "encodings": encodings, "time": json.loads(timing.read_text()),
            "elapsed_seconds": elapsed, "stdout": process.stdout, "stderr": process.stderr}


def aggregate(classifications):
    if "OK" in classifications:
        return "OK"
    if "UNKNOWN" in classifications:
        return "UNKNOWN"
    return "NG"


def validate_candidate_sequence(case, new, old):
    placements = case["start"][:64].count("-") - case["goal"][:64].count("-")
    bound = 2 * placements + int(not moves(case["start"][:64]))
    same_turn = case["start"][-1] == case["goal"][-1]
    needs_encoding = placements >= 0 and any(
        (layer % 2 == 0) == same_turn for layer in range(placements, bound + 1)
    )
    candidates = case["candidates"]
    selected = len(candidates)
    if new["classification"] == "OK":
        selected_symmetry = new["record"].split("\t")[4]
        selected = [symmetry for symmetry, _ in candidates].index(selected_symmetry) + 1
    expected_goals = [goal for _, goal in candidates[:selected]] if needs_encoding else []
    actual_goals = [metric["goal"] for metric in new["encodings"]]
    if actual_goals != expected_goals:
        raise ValueError("new encoded candidates differ in order or count from the specified prefix")
    expected_old_count = len(candidates)
    if old[-1]["classification"] == "OK":
        expected_old_count = [symmetry for symmetry, _ in candidates].index(
            old[-1]["candidate_symmetry"]
        ) + 1
    if len(old) != expected_old_count:
        raise ValueError("baseline did not visit the complete required candidate prefix")
    for measured, (symmetry, goal) in zip(old, candidates):
        expected = [goal] if needs_encoding else []
        if measured["candidate_symmetry"] != symmetry or [
            metric["goal"] for metric in measured["encodings"]
        ] != expected:
            raise ValueError("baseline candidate order or encoding count differs")


def summarize(results):
    def measures(processes):
        encodings = [metric for process in processes for metric in process["encodings"]]
        return {
            "processes": len(processes),
            "cnfs": len(encodings),
            "variables": sum(metric["variables"] for metric in encodings),
            "clauses": sum(metric["clauses"] for metric in encodings),
            "build_seconds": sum(metric["build_seconds"] for metric in encodings),
            "solve_seconds": sum(metric["solve_seconds"] for metric in encodings),
            "elapsed_seconds": sum(process["elapsed_seconds"] for process in processes),
            "max_rss_kib": max(process["time"]["max_rss_kib"] for process in processes),
        }

    counts = {}
    for mode in ("exact", "symmetry"):
        counts[mode] = {"old": dict.fromkeys(CATEGORIES, 0), "new": dict.fromkeys(CATEGORIES, 0)}
        for row in results:
            if row["query"]["mode"] == mode:
                counts[mode]["old"][row["old_classification"]] += 1
                counts[mode]["new"][row["new"]["classification"]] += 1
    return {
        "classification_counts": counts,
        "total": {
            "old": measures([process for row in results for process in row["old_candidates"]]),
            "new": measures([row["new"] for row in results]),
        },
        "queries": [{
            "name": row["query"]["name"], "mode": row["query"]["mode"],
            "old_classification": row["old_classification"],
            "new_classification": row["new"]["classification"],
            "old": measures(row["old_candidates"]), "new": measures([row["new"]]),
        } for row in results],
    }


def compare(args):
    root, artifacts, scratch = args.root.resolve(), args.artifacts.resolve(), args.scratch.resolve()
    baseline = json.loads((artifacts / "baseline.json").read_text())
    old_binary, new_binary = Path(baseline["binary"]), root / "target/release/layer_sat"
    if sha256(old_binary) != baseline["binary_sha256"]:
        raise ValueError("prepared baseline binary changed")
    cases, research = make_inputs(root)
    write_json(artifacts / "inputs.json", {"queries": cases, "research_source": research})
    source_paths = ["Cargo.toml", "Cargo.lock", "src/prunings/layer_sat.rs", "src/bin/layer_sat.rs",
                    "src/othello.rs", "src/io.rs", "src/record.rs"]
    metadata = {
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "command": ["python3", str(Path(__file__).resolve())] + sys.argv[1:],
        "baseline": baseline,
        "new": {"commit": command(["git", "rev-parse", "HEAD"], root).stdout.strip(),
                "binary": str(new_binary), "binary_sha256": sha256(new_binary),
                "source_sha256": {path: sha256(root / path) for path in source_paths},
                "build_command": ["cargo", "build", "--release"], "allocator": "system"},
        "rustc": command(["rustc", "-Vv"]).stdout,
        "cargo": command(["cargo", "-V"]).stdout,
        "cpu": json.loads(command(["lscpu", "-J"]).stdout),
        "kernel": command(["uname", "-a"]).stdout.strip(),
        "solver": "rustsat-kissat 0.7.4, Kissat 4.0.4 (v4-0-4 feature), default options",
        "parallel_goals": 1, "timeout_seconds_per_candidate": args.timeout,
        "environment": {name: os.environ.get(name) for name in (
            "RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "RUSTUP_TOOLCHAIN", "RAYON_NUM_THREADS")},
        "timing_scope": "Var allocation + encode_problem; solve_single_problem including CNF conversion/addition and model decoding, excluding witness replay",
        "memory_scope": "GNU time maximum resident set per process, KiB; baseline symmetry is maximum across sequential candidate processes",
    }
    write_json(artifacts / "metadata.json", metadata)
    results = []
    for case in cases:
        output = scratch / "runs" / case["name"]
        new = execute(new_binary, case["start"], case["goal"], case["mode"], "new", output / "new", args.timeout)
        old = []
        for symmetry, candidate in case["candidates"]:
            measured = execute(old_binary, case["start"], candidate, "exact", "old", output / f"old-{symmetry}", args.timeout)
            measured["candidate_symmetry"] = symmetry
            old.append(measured)
            if measured["classification"] == "OK":
                break
        old_class = aggregate([candidate["classification"] for candidate in old])
        validate_candidate_sequence(case, new, old)
        fixed_disagreements = [outcome for outcome in (new["classification"], old_class)
                               if case["oracle"] is not None and outcome not in (case["oracle"], "UNKNOWN")]
        definite_disagreement = {new["classification"], old_class} == {"OK", "NG"}
        result = {"query": case, "new": new, "old_candidates": old,
                  "old_classification": old_class, "definite_disagreement": definite_disagreement,
                  "oracle_disagreement": bool(fixed_disagreements)}
        results.append(result)
        write_json(artifacts / "results.json", results)
        print(f"{case['name']}: old={old_class}, new={new['classification']}, oracle={case['oracle']}", flush=True)
    if sha256(new_binary) != metadata["new"]["binary_sha256"]:
        raise ValueError("new binary changed during measurement")
    write_json(artifacts / "summary.json", summarize(results))
    if any(row["definite_disagreement"] or row["oracle_disagreement"] for row in results):
        raise ValueError("measurement found a definite disagreement; inspect independent evidence")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="operation", required=True)
    old_parser = subparsers.add_parser("prepare-baseline")
    old_parser.add_argument("--baseline", type=Path, required=True)
    new_parser = subparsers.add_parser("compare")
    new_parser.add_argument("--root", type=Path, required=True)
    new_parser.add_argument("--timeout", type=int, required=True)
    for subparser in (old_parser, new_parser):
        subparser.add_argument("--scratch", type=Path, required=True)
        subparser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    if args.operation == "prepare-baseline":
        prepare(args)
    else:
        if args.timeout <= 0:
            parser.error("--timeout must be positive")
        compare(args)


if __name__ == "__main__":
    main()
