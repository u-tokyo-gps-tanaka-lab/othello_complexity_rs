#!/usr/bin/env python3
"""Check the hand-written ranker gradients against central differences."""

import argparse
import importlib.util
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    path = Path(__file__).with_name("train_reverse_ranker.py")
    spec = importlib.util.spec_from_file_location("ranker_train", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    np = module.np
    rng = np.random.default_rng(2026102509)
    positive = rng.random((7, module.FEATURE_COUNT))
    comparison = rng.random((7, module.FEATURE_COUNT))
    results = []
    for hidden in (0, 16):
        model = module.Ranker(hidden, 2026102509)
        if not hidden:
            model.parameters["input_weights"][:] = rng.normal(0, 0.1, module.FEATURE_COUNT)
        l2 = 0.0001

        def loss():
            difference = model.scores(comparison) - model.scores(positive)
            penalty = sum(0.5 * l2 * np.sum(values * values)
                          for name, values in model.parameters.items() if name != "hidden_bias")
            return float(np.logaddexp(0, difference).mean() + penalty)

        targets = []
        epsilon = 1e-6
        for name, values in model.parameters.items():
            indices = np.linspace(0, values.size - 1, min(12, values.size), dtype=int)
            for index in indices:
                flat = values.ravel()
                original = flat[index]
                flat[index] = original + epsilon
                plus = loss()
                flat[index] = original - epsilon
                minus = loss()
                flat[index] = original
                targets.append((name, int(index), (plus - minus) / (2 * epsilon)))
        model.update(positive, comparison, 0.001, l2)
        errors = []
        for name, index, numerical in targets:
            analytic = float(model.moments[name].ravel()[index] / 0.1)
            errors.append(abs(numerical - analytic))
        maximum = max(errors)
        if maximum >= 2e-8:
            raise AssertionError(f"hidden={hidden}: gradient error {maximum}")
        results.append({"hidden_size": hidden, "checked_parameters": len(errors),
                        "max_absolute_gradient_error": maximum})
    report = {"method": "central finite difference of pairwise loss plus L2; compared with the first Adam moment divided by 1-beta1",
              "epsilon": 1e-6, "tolerance": 2e-8,
              "checks": results, "passed": True}
    if args.output:
        args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
