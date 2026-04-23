#!/usr/bin/env python3
"""Summarize one or more sched-claw metrics.env files.

This helper keeps aggregation choices in the skill layer so agents can pick the
reducer that matches the workload instead of inheriting a fixed host policy.
"""

from __future__ import annotations

import argparse
import math
import statistics
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("metrics_files", nargs="+", type=Path)
    parser.add_argument(
        "--metric",
        action="append",
        dest="metrics",
        help="Limit the summary to one or more metric names.",
    )
    parser.add_argument(
        "--reducer",
        default="median",
        choices=("median", "mean", "min", "max"),
    )
    return parser.parse_args()


def parse_metrics_file(path: Path) -> dict[str, float]:
    metrics: dict[str, float] = {}
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, raw_value = stripped.split("=", 1)
        try:
            value = float(raw_value.strip())
        except ValueError:
            continue
        if math.isfinite(value):
            metrics[key.strip()] = value
    return metrics


def reduce_values(values: list[float], reducer: str) -> float:
    if reducer == "median":
        return statistics.median(values)
    if reducer == "mean":
        return statistics.fmean(values)
    if reducer == "min":
        return min(values)
    if reducer == "max":
        return max(values)
    raise ValueError(f"unsupported reducer {reducer}")


def main() -> int:
    args = parse_args()
    buckets: dict[str, list[float]] = {}
    for path in args.metrics_files:
        for key, value in parse_metrics_file(path).items():
            buckets.setdefault(key, []).append(value)

    metric_names = sorted(args.metrics or buckets.keys())
    for metric in metric_names:
        values = buckets.get(metric, [])
        if not values:
            print(f"{metric}: <missing>")
            continue
        summary = reduce_values(values, args.reducer)
        print(
            f"{metric}: count={len(values)} reducer={args.reducer} "
            f"value={summary:.6f} min={min(values):.6f} max={max(values):.6f} values={values}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
