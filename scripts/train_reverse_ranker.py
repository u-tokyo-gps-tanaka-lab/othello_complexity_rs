#!/usr/bin/env python3
"""Train small ordering models from demonstrated predecessors.

The target is a ranking preference for an observed history. A comparison row is
not labelled unreachable. No classifier output is used as a reachability proof.
Only validation pairwise loss selects an epoch; search holdouts are not read.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import platform
import sys
import time

# Set before importing NumPy so that repeated training and timing are predictable.
for variable in ("OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS", "MKL_NUM_THREADS"):
    os.environ[variable] = "1"

import numpy as np


FEATURE_COUNT = 160
FEATURE_VERSION = "reverse-board-v1"
SCHEMA = "othello-reverse-ranker-v1"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


class Dataset:
    def __init__(self, path: Path):
        self.path = path
        self.sha256 = sha256(path)
        self.groups: list[tuple[int, int, str, int]] = []
        self.positions: list[str] = []
        self.keys: set[str] = set()
        self.games: set[str] = set()
        self.goals: set[str] = set()
        observed = []
        start = 0
        previous = None
        seen_groups = set()
        group_stones = 0
        with path.open(newline="") as stream:
            reader = csv.DictReader(stream)
            expected = [f"f{i}" for i in range(FEATURE_COUNT)]
            if reader.fieldnames is None or reader.fieldnames[9:] != expected:
                raise ValueError(f"{path}: feature columns do not match {FEATURE_VERSION}")
            for row_index, row in enumerate(reader):
                group = row["group_id"]
                if group != previous:
                    if previous is not None:
                        self.groups.append((start, row_index, previous, group_stones))
                    if group in seen_groups:
                        raise ValueError(f"{path}: noncontiguous group {group}")
                    seen_groups.add(group)
                    start = row_index
                    previous = group
                    group_stones = int(row["stones"])
                if row["observed"] not in ("0", "1"):
                    raise ValueError("observed is a demonstration flag, expected 0 or 1")
                observed.append(row["observed"] == "1")
                self.positions.append(row["candidate"])
                self.keys.add(row["canonical_candidate"])
                self.keys.add(row["canonical_goal"])
                self.games.add(row["game_id"])
                self.goals.add(row["goal"])
        if previous is None:
            raise ValueError(f"{path}: no training groups")
        self.groups.append((start, len(observed), previous, group_stones))
        self.observed = np.array(observed, dtype=np.bool_)
        self.x = np.loadtxt(
            path, delimiter=",", skiprows=1, usecols=tuple(range(9, 9 + FEATURE_COUNT)),
            dtype=np.float64, ndmin=2,
        )
        if self.x.shape != (len(observed), FEATURE_COUNT):
            raise ValueError("feature row count mismatch")
        if not np.isfinite(self.x).all() or np.min(self.x) < 0 or np.max(self.x) > 1:
            raise ValueError("features must be finite and bounded in [0,1]")
        positive_indices = []
        comparison_indices = []
        for begin, end, group, _ in self.groups:
            positive = np.flatnonzero(self.observed[begin:end]) + begin
            comparison = np.flatnonzero(~self.observed[begin:end]) + begin
            if len(positive) == 0 or len(comparison) == 0:
                raise ValueError(f"{group}: need an observed and an alternative candidate")
            if len(set(self.positions[begin:end])) != end - begin:
                raise ValueError(f"{group}: duplicate candidate")
            for a in positive:
                for b in comparison:
                    positive_indices.append(a)
                    comparison_indices.append(b)
        self.positive = np.array(positive_indices, dtype=np.int64)
        self.comparison = np.array(comparison_indices, dtype=np.int64)

    def summary(self) -> dict:
        return {
            "sha256": self.sha256,
            "games": len(self.games),
            "groups": len(self.groups),
            "rows": len(self.x),
            "observed_rows": int(self.observed.sum()),
            "comparison_rows": int((~self.observed).sum()),
            "preference_pairs": len(self.positive),
            "canonical_keys": len(self.keys),
        }


class Ranker:
    def __init__(self, hidden: int, seed: int):
        self.hidden = hidden
        rng = np.random.default_rng(seed)
        if hidden:
            self.parameters = {
                "input_weights": rng.normal(0, np.sqrt(2 / FEATURE_COUNT), (hidden, FEATURE_COUNT)),
                "hidden_bias": np.zeros(hidden, dtype=np.float64),
                "output_weights": rng.normal(0, 1 / np.sqrt(hidden), hidden),
            }
        else:
            self.parameters = {"input_weights": np.zeros(FEATURE_COUNT, dtype=np.float64)}
        self.moments = {key: np.zeros_like(value) for key, value in self.parameters.items()}
        self.variances = {key: np.zeros_like(value) for key, value in self.parameters.items()}
        self.steps = 0

    def scores(self, x: np.ndarray) -> np.ndarray:
        if self.hidden:
            hidden = np.maximum(
                x @ self.parameters["input_weights"].T + self.parameters["hidden_bias"], 0
            )
            return hidden @ self.parameters["output_weights"]
        return x @ self.parameters["input_weights"]

    def update(self, positive: np.ndarray, comparison: np.ndarray, lr: float, l2: float) -> None:
        size = len(positive)
        if self.hidden:
            x = np.concatenate((positive, comparison))
            preactivation = x @ self.parameters["input_weights"].T + self.parameters["hidden_bias"]
            hidden = np.maximum(preactivation, 0)
            scores = hidden @ self.parameters["output_weights"]
            difference = scores[size:] - scores[:size]
        else:
            difference_x = comparison - positive
            difference = difference_x @ self.parameters["input_weights"]
        # d/dz softplus(z), written to avoid overflow for large score differences.
        derivative = np.exp(-np.logaddexp(0.0, -difference)) / size
        if self.hidden:
            score_gradient = np.concatenate((-derivative, derivative))
            hidden_gradient = score_gradient[:, None] * self.parameters["output_weights"][None, :]
            hidden_gradient *= preactivation > 0
            gradients = {
                "input_weights": hidden_gradient.T @ x,
                "hidden_bias": hidden_gradient.sum(axis=0),
                "output_weights": hidden.T @ score_gradient,
            }
        else:
            gradients = {"input_weights": difference_x.T @ derivative}
        self.steps += 1
        for name, values in self.parameters.items():
            gradient = gradients[name]
            if name != "hidden_bias":
                gradient = gradient + l2 * values
            self.moments[name] = 0.9 * self.moments[name] + 0.1 * gradient
            self.variances[name] = 0.999 * self.variances[name] + 0.001 * gradient * gradient
            corrected_moment = self.moments[name] / (1 - 0.9 ** self.steps)
            corrected_variance = self.variances[name] / (1 - 0.999 ** self.steps)
            values -= lr * corrected_moment / (np.sqrt(corrected_variance) + 1e-8)
            if not np.isfinite(values).all():
                raise FloatingPointError("nonfinite model parameter")

    def export(self, training: dict) -> dict:
        return {
            "schema": SCHEMA,
            "feature_version": FEATURE_VERSION,
            "feature_count": FEATURE_COUNT,
            "hidden_size": self.hidden,
            "input_weights": self.parameters["input_weights"].ravel().tolist(),
            "hidden_bias": self.parameters.get("hidden_bias", np.array([])).tolist(),
            "output_weights": self.parameters.get("output_weights", np.array([])).tolist(),
            # A common score offset cancels exactly in a pairwise ordering loss.
            "output_bias": 0.0,
            "training": training,
        }


def evaluate(model: Ranker, data: Dataset) -> dict:
    score = np.concatenate([model.scores(data.x[i:i + 8192]) for i in range(0, len(data.x), 8192)])
    difference = score[data.positive] - score[data.comparison]
    loss = np.logaddexp(0.0, -difference).mean()
    pair_accuracy = np.mean((difference > 0) + 0.5 * (difference == 0))
    ranks = []
    by_stones: dict[int, list[int]] = {}
    for begin, end, _, stones in data.groups:
        # A deterministic position-string tie break avoids granting a tied model
        # a spurious top-1 success just because demonstrations were written first.
        ordering = sorted(range(begin, end), key=lambda i: (-float(score[i]), data.positions[i]))
        rank = next(index + 1 for index, row in enumerate(ordering) if data.observed[row])
        ranks.append(rank)
        by_stones.setdefault(stones, []).append(rank)
    return {
        "pairwise_logistic_loss": float(loss),
        "observed_vs_comparison_pair_accuracy": float(pair_accuracy),
        "demonstrated_predecessor_top1": float(np.mean(np.array(ranks) <= 1)),
        "demonstrated_predecessor_top3": float(np.mean(np.array(ranks) <= 3)),
        "mean_demonstrated_predecessor_rank": float(np.mean(ranks)),
        "group_count": len(ranks),
        "by_stones": {
            str(stones): {"groups": len(values), "top1": float(np.mean(np.array(values) == 1)),
                          "mean_rank": float(np.mean(values))}
            for stones, values in sorted(by_stones.items())
        },
    }


def train(hidden: int, train_data: Dataset, validation: Dataset, args: argparse.Namespace) -> tuple[Ranker, dict, list[dict]]:
    seed = args.seed + hidden
    model = Ranker(hidden, seed)
    random = np.random.default_rng(seed ^ 0x6C6561726E)
    lr = 0.001 if hidden else 0.003
    best_loss = float("inf")
    best = None
    best_epoch = 0
    curve = []
    for epoch in range(1, args.epochs + 1):
        order = random.permutation(len(train_data.positive))
        for start in range(0, len(order), args.batch_size):
            batch = order[start:start + args.batch_size]
            model.update(train_data.x[train_data.positive[batch]],
                         train_data.x[train_data.comparison[batch]], lr, args.l2)
        train_metrics = evaluate(model, train_data)
        validation_metrics = evaluate(model, validation)
        curve.append({"epoch": epoch, "train": train_metrics, "validation": validation_metrics})
        if validation_metrics["pairwise_logistic_loss"] < best_loss:
            best_loss = validation_metrics["pairwise_logistic_loss"]
            best = {name: values.copy() for name, values in model.parameters.items()}
            best_epoch = epoch
        print(f"hidden={hidden} epoch={epoch} train_loss={train_metrics['pairwise_logistic_loss']:.6f} "
              f"validation_loss={validation_metrics['pairwise_logistic_loss']:.6f} "
              f"validation_top1={validation_metrics['demonstrated_predecessor_top1']:.4f}", flush=True)
    assert best is not None
    model.parameters = best
    metadata = {
        "objective": "pairwise logistic preference for demonstrated predecessors; comparison candidates are not labelled unreachable",
        "seed": seed,
        "optimizer": "Adam beta1=0.9 beta2=0.999 epsilon=1e-8",
        "learning_rate": lr,
        "l2": args.l2,
        "batch_size": args.batch_size,
        "epochs_trained": args.epochs,
        "selected_epoch": best_epoch,
        "selection_rule": "minimum validation pairwise logistic loss; no search holdout used",
        "train": train_data.summary(),
        "validation": validation.summary(),
        "train_metrics": evaluate(model, train_data),
        "validation_metrics": evaluate(model, validation),
        "metric_scope": "sampled comparison groups only; these ranking metrics are not reachability accuracy or search success rates",
    }
    return model, metadata, curve


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--train", type=Path, required=True)
    parser.add_argument("--validation", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--seed", type=int, default=2026102503)
    parser.add_argument("--epochs", type=int, default=30)
    parser.add_argument("--batch-size", type=int, default=1024)
    parser.add_argument("--l2", type=float, default=1e-4)
    args = parser.parse_args()
    if args.epochs <= 0 or args.batch_size <= 0 or args.l2 < 0:
        parser.error("epochs and batch-size must be positive; l2 must be nonnegative")
    args.out_dir.mkdir(parents=True, exist_ok=True)
    for filename in ("linear.json", "mlp16.json", "training_metrics.json", "training_curve.json", "inference_parity.json"):
        if (args.out_dir / filename).exists():
            raise FileExistsError(f"refusing to overwrite {args.out_dir / filename}")
    started = time.monotonic()
    train_data = Dataset(args.train)
    validation = Dataset(args.validation)
    overlapping_games = train_data.games & validation.games
    overlapping_positions = train_data.keys & validation.keys
    if overlapping_games or overlapping_positions:
        raise ValueError(f"split leakage: {len(overlapping_games)} games and {len(overlapping_positions)} canonical keys overlap")
    models = {}
    metrics = {}
    curves = {}
    for hidden, name in ((0, "linear"), (16, "mlp16")):
        model, metadata, curve = train(hidden, train_data, validation, args)
        models[name] = model
        metrics[name] = metadata
        curves[name] = curve
        write_json(args.out_dir / f"{name}.json", model.export(metadata))
    parity = []
    for source, data in (("train", train_data), ("validation", validation)):
        indices = np.linspace(0, len(data.x) - 1, 16, dtype=int)
        for index in indices:
            parity.append({
                "split": source,
                "position": data.positions[index],
                "features": data.x[index].tolist(),
                "scores": {name: float(model.scores(data.x[index:index + 1])[0]) for name, model in models.items()},
            })
    write_json(args.out_dir / "inference_parity.json", {
        "feature_version": FEATURE_VERSION, "cases": parity,
        "purpose": "Rust encoder and exported-parameter inference must agree with these NumPy values; not a reachability oracle",
    })
    write_json(args.out_dir / "training_curve.json", curves)
    write_json(args.out_dir / "training_metrics.json", {
        "schema": "othello-reverse-ranker-training-report-v1",
        "python": platform.python_version(),
        "numpy": np.__version__,
        "blas_threads": 1,
        "elapsed_seconds": time.monotonic() - started,
        "training_script_sha256": sha256(Path(__file__)),
        "split_check": {"overlapping_games": 0, "overlapping_canonical_candidate_or_goal_positions": 0},
        "models": metrics,
        "model_sha256": {name: sha256(args.out_dir / f"{name}.json") for name in models},
    })
    print(json.dumps({"out_dir": str(args.out_dir), "elapsed_seconds": time.monotonic() - started}), flush=True)


if __name__ == "__main__":
    main()
