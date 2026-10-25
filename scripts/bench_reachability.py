#!/usr/bin/env python3
"""Reproducible A/B benchmarks with independent certificate verification.

Only the Python standard library is required. Build each revision in release
mode, then pass the two *binaries*, not cargo commands. Example:

  python3 scripts/bench_reachability.py generate --out /tmp/corpus.json
  python3 scripts/bench_reachability.py compare --corpus /tmp/corpus.json \
    --baseline /tmp/baseline/sat_reachability \
    --candidate target/release/sat_reachability --out /tmp/benchmark

The corpus is for performance/regression checks, not population estimation.
Random play does not sample reachable positions uniformly. Unknown labels are
deliberate: mutating a reachable board does not prove it unreachable. Every SAT
certificate is replayed with coordinate-based rules independent of Rust code.
UNSAT is checked against known witnesses, the shallow exhaustive oracle, and
the other revision; this is not a general UNSAT proof checker.
"""

import argparse
import collections
import csv
import hashlib
import json
import platform
from pathlib import Path
import random
import re
import statistics
import subprocess
import sys
import threading
import time


INITIAL = "---------------------------OX------XO--------------------------- b"
MODES = ("exact", "symmetry", "equivalence")
SYMMETRIES = (
    "identity", "rotate90", "rotate180", "rotate270", "mirror_horizontal",
    "mirror_vertical", "transpose", "anti_transpose",
)


def parse_position(text):
    if not re.fullmatch(r"[XO-]{64} [bw]", text):
        raise ValueError(f"invalid position: {text!r}")
    turn = 1 if text[-1] == "b" else -1
    cells = tuple(0 if c == "-" else turn if c == "X" else -turn for c in text[:64])
    if any(cells[s] == 0 for s in (27, 28, 35, 36)):
        raise ValueError("the four center squares must be occupied")
    return cells, turn


def format_position(position):
    cells, turn = position
    return "".join("-" if c == 0 else "X" if c == turn else "O" for c in cells) + (
        " b" if turn == 1 else " w"
    )


def captures(position, square):
    cells, turn = position
    if square not in range(64) or cells[square] != 0:
        return []
    flipped = []
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            if (dx, dy) == (0, 0):
                continue
            x, y = square % 8 + dx, square // 8 + dy
            line = []
            while 0 <= x < 8 and 0 <= y < 8:
                value = cells[8 * y + x]
                if value == -turn:
                    line.append(8 * y + x)
                else:
                    if value == turn:
                        flipped.extend(line)
                    break
                x, y = x + dx, y + dy
    return flipped


def legal_moves(position):
    return [s for s in range(64) if captures(position, s)]


def apply(position, action):
    cells, turn = position
    if action == "ps":
        if legal_moves(position) or not legal_moves((cells, -turn)):
            raise ValueError("illegal pass, including a pass after termination")
        return cells, -turn
    if not re.fullmatch(r"[A-H][1-8]", action):
        raise ValueError(f"invalid action: {action!r}")
    square = ord(action[0]) - ord("A") + 8 * (int(action[1]) - 1)
    flipped = captures(position, square)
    if not flipped:
        raise ValueError(f"illegal placement: {action}")
    after = list(cells)
    for s in [square, *flipped]:
        after[s] = turn
    return tuple(after), -turn


def coordinate(square):
    return chr(ord("A") + square % 8) + str(square // 8 + 1)


def transform(position, name):
    symmetry, *suffix = name.split("+")
    if symmetry not in SYMMETRIES or suffix not in ([], ["color_exchange"]):
        raise ValueError(f"invalid transform: {name!r}")
    cells, turn = position
    mapped = [0] * 64
    for s, value in enumerate(cells):
        x, y = s % 8, s // 8
        u, v = (
            (x, y), (7 - y, x), (7 - x, 7 - y), (y, 7 - x),
            (7 - x, y), (x, 7 - y), (y, x), (7 - y, 7 - x),
        )[SYMMETRIES.index(symmetry)]
        mapped[8 * v + u] = -value if suffix else value
    return tuple(mapped), -turn if suffix else turn


def images(position, mode):
    for symmetry in SYMMETRIES if mode != "exact" else ("identity",):
        yield transform(position, symmetry)
        if mode == "equivalence":
            yield transform(position, symmetry + "+color_exchange")


def enumerate_reachable(start, max_discs):
    seen, stack = set(), [start]
    while stack:
        position = stack.pop()
        if position in seen:
            continue
        seen.add(position)
        moves = legal_moves(position)
        if moves and sum(c != 0 for c in position[0]) < max_discs:
            stack.extend(apply(position, coordinate(s)) for s in moves)
        elif not moves and legal_moves((position[0], -position[1])):
            stack.append(apply(position, "ps"))
    return seen


def oracle_labels(goal, reachable):
    return {mode: any(g in reachable for g in images(goal, mode)) for mode in MODES}


def generate(args):
    rng = random.Random(args.seed)
    initial = parse_position(INITIAL)
    shallow = enumerate_reachable(initial, 8)
    cases = []

    def add(start, goal, kind, expected):
        cases.append({"start": format_position(start), "goal": format_position(goal),
                      "kind": kind, "expected": expected})

    for discs in args.discs:
        for _ in range(args.count):
            # Restart the rare game that terminates before this disc count.
            while True:
                position = initial
                while sum(c != 0 for c in position[0]) < discs:
                    moves = legal_moves(position)
                    if moves:
                        position = apply(position, coordinate(rng.choice(moves)))
                    elif legal_moves((position[0], -position[1])):
                        position = apply(position, "ps")
                    else:
                        break
                if sum(c != 0 for c in position[0]) == discs:
                    break
            add(initial, position, f"legal-d{discs}", dict.fromkeys(MODES, True))
            changed = list(position[0])
            square = rng.choice([s for s in range(64) if changed[s]])
            changed[square] *= -1
            mutated = tuple(changed), position[1]
            labels = oracle_labels(mutated, shallow) if discs <= 8 else dict.fromkeys(MODES, None)
            add(initial, mutated, f"mutated-d{discs}", labels)
            occupied = {27, 28, 35, 36}
            occupied.update(rng.sample([s for s in range(64) if s not in occupied], discs - 4))
            random_goal = tuple(rng.choice((-1, 1)) if s in occupied else 0 for s in range(64)), rng.choice((-1, 1))
            labels = oracle_labels(random_goal, shallow) if discs <= 8 else dict.fromkeys(MODES, None)
            add(initial, random_goal, f"random-d{discs}", labels)

    # Absolute turns, full terminals, and leading/middle/trailing forced passes.
    # All goals below have exhaustive independent labels, including wrong turns.
    fixtures = [
        "OOOOOOOOXOOXOOO-XXXOXOOOXXXOOOOOXOXOXOXOXOOXXOOOOOOOOOO-OOOOOOOO w",
        "-XOOOOOO-XOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOO w",
    ]
    for fixture_index, fixture in enumerate(fixtures):
        start = parse_position(fixture)
        reachable = enumerate_reachable(start, 64)
        goals = reachable | {(cells, -turn) for cells, turn in reachable}
        for goal in sorted(goals, key=format_position):
            add(start, goal, f"forced-pass-{fixture_index}", oracle_labels(goal, reachable))

    corpus = {"format": "reachability-benchmark-v1", "seed": args.seed,
              "discs": args.discs, "count_per_kind": args.count, "cases": cases}
    if args.historical_lp_survivors:
        source = args.historical_lp_survivors
        lines = source.read_text().splitlines()
        if any(not re.fullmatch(r"[XO-]{64}", line) for line in lines):
            raise ValueError("historical input must contain 64-character relative boards")
        selection_rng = random.Random(args.seed)
        for band in (30, 40, 50):
            candidates = [(i + 1, line) for i, line in enumerate(lines)
                          if band <= sum(c in "XO" for c in line) < band + 10]
            for source_line, line in selection_rng.sample(candidates, min(args.survivors_per_band, len(candidates))):
                # A benchmark query is defined here; old reachability labels
                # are never inherited, even if that source contains them.
                add(initial, parse_position(line + " b"), f"historical-lp-survivor-band{band}",
                    dict.fromkeys(MODES, None))
                cases[-1]["historical_source_line"] = source_line
        corpus["historical_source"] = {
            "path": str(source), "sha256": sha256(source), "selection_seed": args.seed,
            "max_per_decade_band": args.survivors_per_band, "relative_board_turn": "b",
            "note": "Historical 64-character relative boards are assigned Black to move for this benchmark; "
                    "historical reachability labels are not reused. Passing LP is not a reachability proof.",
        }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(corpus, indent=2) + "\n")
    print(f"Wrote {len(cases)} cases to {args.out}")


def validate_records(path, cases, statuses, mode):
    verified = set()
    with path.open(newline="") as stream:
        for fields in csv.reader(stream):
            if len(fields) != 6:
                raise ValueError(f"{path}: record does not have six columns")
            index_text, start_text, goal_text, record_mode, name, actions = fields
            index = int(index_text) - 1
            if index not in range(len(cases)) or index in verified:
                raise ValueError(f"{path}: invalid or duplicate record index {index + 1}")
            case = cases[index]
            if (start_text, goal_text, record_mode) != (case["start"], case["goal"], mode):
                raise ValueError(f"{path}: certificate does not match input {index + 1}")
            if statuses[index] != "OK":
                raise ValueError(f"{path}: certificate supplied for non-OK input")
            if mode == "exact" and name != "identity" or mode == "symmetry" and "+" in name:
                raise ValueError(f"{path}: disallowed transform")
            position = parse_position(start_text)
            for action in actions.split():
                position = apply(position, action)
            if position != transform(parse_position(goal_text), name):
                raise ValueError(f"{path}: wrong certificate endpoint for input {index + 1}")
            verified.add(index)
    if verified != {i for i, status in enumerate(statuses) if status == "OK"}:
        raise ValueError(f"{path}: an OK classification lacks a certificate")
    return len(verified)


def run_group(args, binary, cases, out_dir):
    out_dir.mkdir(parents=True)
    goals = out_dir / "input.txt"
    goals.write_text("".join(case["goal"] + "\n" for case in cases))
    if args.engine == "sat":
        command = [str(binary), "--start", cases[0]["start"], "--goal-file", str(goals),
                   "--mode", args.mode, "--encoding", args.encoding, "--parallel-goals",
                   str(args.workers), "--sat-timeout-secs", str(args.timeout), "--verbose",
                   "--out-dir", str(out_dir)]
        pattern = r"^\[(\d+)\] (SAT|UNSAT|UNKNOWN)(?:\s|$)"
        statuses_map = {"SAT": "OK", "UNSAT": "NG", "UNKNOWN": "UNKNOWN"}
        prefix = f"sat_{args.mode}"
    else:
        command = [str(binary), args.strategy, str(goals), "--mode", args.mode,
                   "--discs", str(args.forward_discs), "--max-nodes", str(args.max_nodes),
                   "--out-dir", str(out_dir)]
        if args.strategy in ("dfs-parallel", "gbfs-parallel"):
            command.extend(["--threads", str(args.workers)])
        if args.strategy == "dfs-parallel":
            command.extend(["--table-size", str(args.table_size)])
        pattern = r"^input (\d+): (OK|NG|UNKNOWN) nodes=(\d+)"
        statuses_map = {s: s for s in ("OK", "NG", "UNKNOWN")}
        prefix = f"reverse_{args.mode}"
    started = time.perf_counter()
    with (out_dir / "stdout.log").open("w") as stdout, (out_dir / "stderr.log").open("w") as stderr:
        # Popen.wait(timeout=...) polls in up-to-50-ms sleep intervals on POSIX,
        # which distorts short reverse-search timings. Block in waitpid instead
        # and enforce the independent emergency cap with a cancellable timer.
        with subprocess.Popen(command, stdout=stdout, stderr=stderr) as process:
            expired = threading.Event()

            def stop_process():
                if process.poll() is None:
                    expired.set()
                    process.kill()

            timer = threading.Timer(args.process_timeout, stop_process)
            timer.daemon = True
            timer.start()
            try:
                returncode = process.wait()
            finally:
                timer.cancel()
                timer.join()
            if expired.is_set():
                raise subprocess.TimeoutExpired(command, args.process_timeout)
            if returncode:
                raise subprocess.CalledProcessError(returncode, command)
    elapsed = time.perf_counter() - started
    log = (out_dir / "stdout.log").read_text()
    statuses = [None] * len(cases)
    nodes = 0
    node_counts = [None] * len(cases)
    for match in re.finditer(pattern, log, re.MULTILINE):
        index = int(match[1]) - 1
        if index not in range(len(cases)) or statuses[index] is not None:
            raise ValueError(f"invalid/duplicate classification index: {match[1]}")
        statuses[index] = statuses_map[match[2]]
        if args.engine == "reverse":
            node_counts[index] = int(match[3])
            nodes += node_counts[index]
    if any(status is None for status in statuses):
        raise ValueError(f"{out_dir}: missing classifications (execution failure, not NG)")
    for status in ("OK", "NG", "UNKNOWN"):
        expected = collections.Counter(c["goal"] for c, s in zip(cases, statuses) if s == status)
        actual = collections.Counter((out_dir / f"{prefix}_{status}.txt").read_text().splitlines())
        if actual != expected:
            raise ValueError(f"{out_dir}: {status} output does not preserve input multiplicity")
    certificates = validate_records(out_dir / f"{prefix}_records.csv", cases, statuses, args.mode)
    for case, status in zip(cases, statuses):
        expected = case.get("expected", {}).get(args.mode)
        if expected is True and status == "NG" or expected is False and status == "OK":
            raise ValueError(f"independent oracle contradiction: {case} -> {status}")
    metrics = re.findall(r"vars=(\d+) clauses=(\d+) build_secs=([\d.]+) solve_secs=([\d.]+)", log)
    return {"elapsed_seconds": elapsed, "statuses": statuses, "verified_records": certificates,
            "candidate_calls": len(metrics), "variables_sum": sum(int(m[0]) for m in metrics),
            "clauses_sum": sum(int(m[1]) for m in metrics),
            "build_seconds_sum": sum(float(m[2]) for m in metrics),
            "solve_seconds_sum": sum(float(m[3]) for m in metrics), "nodes": nodes,
            "node_counts": node_counts,
            "command": command}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def compare(args):
    corpus_text = args.corpus.read_text()
    if args.corpus.suffix == ".json":
        corpus = json.loads(corpus_text)
        if corpus.get("format") != "reachability-benchmark-v1":
            raise ValueError("unsupported corpus format")
        cases = corpus["cases"]
    else:
        cases = [{"start": INITIAL, "goal": line, "kind": "external", "expected": {}}
                 for line in corpus_text.splitlines()]
    for case in cases:
        parse_position(case["start"])
        parse_position(case["goal"])
    cases = [c for c in cases if re.search(args.group, c["kind"])]
    skipped = 0
    if args.engine == "reverse":
        skipped = sum(c["start"] != INITIAL for c in cases)
        cases = [c for c in cases if c["start"] == INITIAL]
    if args.limit:
        cases = cases[:args.limit]
    if not cases:
        raise ValueError("no selected cases")
    binaries = {"baseline": args.baseline.resolve(), "candidate": args.candidate.resolve()}
    binary_hashes = {name: sha256(path) for name, path in binaries.items()}
    if binary_hashes["baseline"] == binary_hashes["candidate"]:
        raise ValueError("baseline and candidate binaries have identical SHA-256 hashes; "
                         "build different revisions in separate Cargo target directories")
    args.out.mkdir(parents=True, exist_ok=False)
    groups = collections.defaultdict(list)
    for index, case in enumerate(cases):
        groups[case["start"]].append((index, case))
    report = {"format": "reachability-benchmark-result-v1", "platform": platform.platform(),
              "python": platform.python_version(), "corpus_sha256": sha256(args.corpus),
              "harness_sha256": sha256(Path(__file__)),
              "timing_method": "blocking-waitpid-with-separate-deadline",
              "binary_sha256": binary_hashes,
              "settings": {key: str(value) if isinstance(value, Path) else value
                           for key, value in vars(args).items() if key != "func"},
              "cases": cases, "skipped_noninitial_cases": skipped, "runs": []}
    ever_decided = {}
    for repetition in range(args.repeat):
        # Alternate order to reduce systematic temperature/cache/order bias.
        for name in ("baseline", "candidate") if repetition % 2 == 0 else ("candidate", "baseline"):
            run = {"revision": name, "repetition": repetition + 1, "groups": [],
                   "elapsed_seconds": 0.0, "statuses": [None] * len(cases)}
            for group_index, members in enumerate(groups.values()):
                directory = args.out / f"{repetition + 1}-{name}" / str(group_index)
                result = run_group(args, binaries[name], [case for _, case in members], directory)
                run["elapsed_seconds"] += result["elapsed_seconds"]
                for (index, _), status in zip(members, result["statuses"]):
                    run["statuses"][index] = status
                    if status != "UNKNOWN":
                        if index in ever_decided and ever_decided[index] != status:
                            raise ValueError(f"OK/NG contradiction across revisions/repeats at case {index + 1}")
                        ever_decided[index] = status
                run["groups"].append(result)
            run["counts"] = dict(collections.Counter(run["statuses"]))
            report["runs"].append(run)
            (args.out / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            print(f"{name} repeat {repetition + 1}: {run['elapsed_seconds']:.3f} s, {run['counts']}", flush=True)
    medians = {name: statistics.median(r["elapsed_seconds"] for r in report["runs"] if r["revision"] == name)
               for name in binaries}
    transitions = []
    for repetition in range(1, args.repeat + 1):
        pair = {r["revision"]: r for r in report["runs"] if r["repetition"] == repetition}
        transitions.append(dict(collections.Counter(
            f"{a}->{b}" for a, b in zip(pair["baseline"]["statuses"], pair["candidate"]["statuses"])
        )))
    report["summary"] = {"median_seconds": medians,
                         "baseline_over_candidate": medians["baseline"] / medians["candidate"],
                         "transitions_by_repeat": transitions,
                         "ok_ng_contradictions": 0}
    (args.out / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report["summary"], indent=2))


def check_results(args):
    """Check all decided answers across revisions, encodings and engines."""
    decisions = {}
    runs_checked = occurrences = 0
    for path in args.results:
        report = json.loads(path.read_text())
        if report.get("format") != "reachability-benchmark-result-v1" or "summary" not in report:
            raise ValueError(f"{path}: not a completed benchmark report")
        mode = report["settings"]["mode"]
        for run in report["runs"]:
            statuses = run["statuses"]
            if len(statuses) != len(report["cases"]):
                raise ValueError(f"{path}: incomplete status list")
            runs_checked += 1
            occurrences += len(statuses)
            for case, status in zip(report["cases"], statuses):
                if status not in ("OK", "NG", "UNKNOWN"):
                    raise ValueError(f"{path}: invalid classification {status!r}")
                if status == "UNKNOWN":
                    continue
                key = (case["start"], case["goal"], mode)
                if key in decisions and decisions[key][0] != status:
                    raise ValueError(f"OK/NG contradiction between {decisions[key][1]} and {path}: {key}")
                decisions[key] = status, str(path)
    print(json.dumps({"reports": len(args.results), "runs_checked": runs_checked,
                      "input_occurrences": occurrences, "unique_decided_queries": len(decisions),
                      "ok_ng_contradictions": 0}, indent=2))


def positive(text):
    value = int(text)
    if value < 1:
        raise argparse.ArgumentTypeError("must be positive")
    return value


def disc_list(text):
    values = [int(n) for n in text.split(",")]
    if not values or any(n < 4 or n > 64 for n in values):
        raise argparse.ArgumentTypeError("disc counts must lie in 4..64")
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    gen = commands.add_parser("generate", help="create deterministic cases with independent known labels")
    gen.add_argument("--out", type=Path, required=True)
    gen.add_argument("--seed", type=int, default=20261026)
    gen.add_argument("--discs", type=disc_list, default=[8, 12, 16, 20, 24])
    gen.add_argument("--count", type=positive, default=3, help="cases per disc count per legal/mutated/random kind")
    gen.add_argument("--historical-lp-survivors", type=Path,
                     help="optional 64-character relative board file; define benchmark queries with Black to move")
    gen.add_argument("--survivors-per-band", type=positive, default=4,
                     help="sample at most this many historical boards in each of the 30s, 40s, 50s disc bands")
    gen.set_defaults(func=generate)
    comp = commands.add_parser("compare", help="run binaries sequentially and reject correctness contradictions")
    comp.add_argument("--corpus", type=Path, required=True, help="generated JSON or existing 66-character positions file")
    comp.add_argument("--baseline", type=Path, required=True)
    comp.add_argument("--candidate", type=Path, required=True)
    comp.add_argument("--out", type=Path, required=True, help="new output directory")
    comp.add_argument("--engine", choices=("sat", "reverse"), default="sat")
    comp.add_argument("--strategy", choices=("dfs", "dfs-move-ordering", "dfs-parallel", "gbfs-parallel"),
                      default="dfs", help="reverse-search strategy")
    comp.add_argument("--mode", choices=MODES, default="exact")
    comp.add_argument("--encoding", choices=("ply", "placement", "placement-columns"), default="placement-columns")
    comp.add_argument("--repeat", type=positive, default=3)
    comp.add_argument("--workers", type=positive, default=1)
    comp.add_argument("--timeout", type=positive, default=1, help="SAT seconds per exact endpoint, excluding CNF construction")
    comp.add_argument("--process-timeout", type=positive, default=600, help="hard cap per batch; expiry is an error")
    comp.add_argument("--group", default=".*", help="regular expression selecting case kinds")
    comp.add_argument("--limit", type=positive)
    comp.add_argument("--forward-discs", type=positive, default=8)
    comp.add_argument("--max-nodes", type=positive, default=10000)
    comp.add_argument("--table-size", type=positive, default=100000)
    comp.set_defaults(func=compare)
    check = commands.add_parser("check-results", help="reject decided-answer contradictions across completed reports")
    check.add_argument("results", type=Path, nargs="+")
    check.set_defaults(func=check_results)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        sys.exit(1)
