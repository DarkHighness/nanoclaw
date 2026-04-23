#!/usr/bin/env python3
"""Compare repeated baseline and candidate runs from a sched-claw experiment.

This helper is intentionally skill-local. It gives the model a fast way to
summarize repeated trials without baking one reducer or anomaly method into the
host substrate.
"""

from __future__ import annotations

import argparse
import math
import statistics
import tomllib
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest", type=Path, help="Path to experiment.toml")
    parser.add_argument("--candidate-id", required=True, help="Candidate to compare")
    parser.add_argument(
        "--metric",
        help="Metric name to compare. Defaults to the manifest primary metric.",
    )
    parser.add_argument(
        "--goal",
        choices=("minimize", "maximize"),
        help="Override the metric goal when the metric is not the manifest primary.",
    )
    parser.add_argument(
        "--baseline-reducer",
        default="median",
        choices=("median", "mean", "min", "max"),
    )
    parser.add_argument(
        "--candidate-reducer",
        default="median",
        choices=("median", "mean", "min", "max"),
    )
    parser.add_argument(
        "--outlier-method",
        default="none",
        choices=("none", "mad", "iqr"),
        help="Optional outlier counter for both baseline and candidate runs.",
    )
    parser.add_argument(
        "--outlier-threshold",
        type=float,
        default=3.5,
        help="Threshold for the chosen outlier method.",
    )
    return parser.parse_args()


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


def pct_improvement(goal: str, baseline: float, candidate: float) -> float | None:
    if baseline == 0:
        return None
    if goal == "minimize":
        return ((baseline - candidate) / baseline) * 100.0
    return ((candidate - baseline) / baseline) * 100.0


def count_outliers(values: list[float], method: str, threshold: float) -> int | None:
    if method == "none":
        return None
    if len(values) < 3:
        return 0
    if method == "mad":
        median = statistics.median(values)
        deviations = [abs(value - median) for value in values]
        mad = statistics.median(deviations)
        if mad == 0:
            return sum(1 for value in values if value != median)
        return sum(
            1
            for value in values
            if 0.6745 * (abs(value - median) / mad) > threshold
        )
    if method == "iqr":
        sorted_values = sorted(values)
        q1 = percentile(sorted_values, 25.0)
        q3 = percentile(sorted_values, 75.0)
        iqr = q3 - q1
        lower = q1 - threshold * iqr
        upper = q3 + threshold * iqr
        return sum(1 for value in values if value < lower or value > upper)
    raise ValueError(f"unsupported outlier method {method}")


def percentile(sorted_values: list[float], pct: float) -> float:
    if not sorted_values:
        raise ValueError("percentile requires at least one value")
    if len(sorted_values) == 1:
        return sorted_values[0]
    position = (len(sorted_values) - 1) * (pct / 100.0)
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return sorted_values[lower]
    weight = position - lower
    return sorted_values[lower] * (1 - weight) + sorted_values[upper] * weight


def extract_metric_values(runs: list[dict], metric: str) -> list[float]:
    values: list[float] = []
    for run in runs:
        raw = run.get("metrics", {}).get(metric)
        if isinstance(raw, (int, float)) and math.isfinite(raw):
            values.append(float(raw))
    return values


def resolve_goal(manifest: dict, metric: str, override: str | None) -> str:
    if override:
        return override
    primary = manifest.get("primary_metric", {})
    if primary.get("name") == metric:
        return primary.get("goal", "minimize")
    for guardrail in manifest.get("guardrails", []):
        if guardrail.get("name") == metric:
            return guardrail.get("goal", "minimize")
    return "minimize"


def main() -> int:
    args = parse_args()
    manifest = tomllib.loads(args.manifest.read_text())
    metric = args.metric or manifest["primary_metric"]["name"]
    goal = resolve_goal(manifest, metric, args.goal)

    baseline_runs = manifest.get("baseline_runs", [])
    candidates = manifest.get("candidates", [])
    candidate = next(
        (
            item
            for item in candidates
            if item.get("spec", {}).get("candidate_id") == args.candidate_id
        ),
        None,
    )
    if candidate is None:
        raise SystemExit(f"unknown candidate {args.candidate_id}")

    baseline_values = extract_metric_values(baseline_runs, metric)
    candidate_values = extract_metric_values(candidate.get("runs", []), metric)
    if not baseline_values:
        raise SystemExit(f"no baseline values found for metric {metric}")
    if not candidate_values:
        raise SystemExit(f"no candidate values found for metric {metric}")

    baseline_summary = reduce_values(baseline_values, args.baseline_reducer)
    candidate_summary = reduce_values(candidate_values, args.candidate_reducer)
    improvement = pct_improvement(goal, baseline_summary, candidate_summary)
    baseline_outliers = count_outliers(
        baseline_values, args.outlier_method, args.outlier_threshold
    )
    candidate_outliers = count_outliers(
        candidate_values, args.outlier_method, args.outlier_threshold
    )

    print(f"experiment: {manifest['experiment_id']}")
    print(f"candidate: {args.candidate_id}")
    print(f"metric: {metric}")
    print(f"goal: {goal}")
    print(
        f"baseline: count={len(baseline_values)} reducer={args.baseline_reducer} "
        f"value={baseline_summary:.6f} values={baseline_values}"
    )
    print(
        f"candidate: count={len(candidate_values)} reducer={args.candidate_reducer} "
        f"value={candidate_summary:.6f} values={candidate_values}"
    )
    if improvement is None:
        print("improvement_pct: <undefined>")
    else:
        print(f"improvement_pct: {improvement:.2f}")
    if args.outlier_method != "none":
        print(
            f"outliers: method={args.outlier_method} threshold={args.outlier_threshold} "
            f"baseline={baseline_outliers} candidate={candidate_outliers}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
