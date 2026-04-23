#!/usr/bin/env python3
"""Compare repeated baseline and candidate runs for sched-claw evaluation.

This helper stays in the skill layer on purpose. It accepts either the older
manifest-style experiment record or direct `metrics.env` files so agents can
choose their own durable artifact shape without inheriting a host-owned scoring
workflow.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
import tomllib
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "manifest",
        nargs="?",
        type=Path,
        help="Optional path to an experiment.toml-like manifest.",
    )
    parser.add_argument("--candidate-id", required=True, help="Candidate to compare")
    parser.add_argument(
        "--baseline-file",
        action="append",
        default=[],
        dest="baseline_files",
        type=Path,
        help="A metrics.env file belonging to a baseline run. Repeat as needed.",
    )
    parser.add_argument(
        "--candidate-file",
        action="append",
        default=[],
        dest="candidate_files",
        type=Path,
        help="A metrics.env file belonging to a candidate run. Repeat as needed.",
    )
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
    parser.add_argument(
        "--out-json",
        type=Path,
        help="Optional path to write a JSON summary.",
    )
    parser.add_argument(
        "--out-markdown",
        type=Path,
        help="Optional path to write a Markdown summary.",
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


def extract_metric_values_from_files(paths: list[Path], metric: str) -> list[float]:
    values: list[float] = []
    for path in paths:
        raw = parse_metrics_file(path).get(metric)
        if raw is not None and math.isfinite(raw):
            values.append(float(raw))
    return values


def render_markdown(result: dict[str, object]) -> str:
    lines = [
        f"# trial comparison: {result['candidate_id']}",
        "",
        "## Summary",
        f"- mode: `{result['mode']}`",
        f"- metric: `{result['metric']}`",
        f"- goal: `{result['goal']}`",
    ]
    improvement = result.get("improvement_pct")
    if isinstance(improvement, (int, float)):
        lines.append(f"- improvement_pct: `{improvement:.2f}`")
    else:
        lines.append("- improvement_pct: `<undefined>`")
    lines.extend(
        [
            "",
            "## Baseline",
            (
                f"- count: `{result['baseline_count']}`"
                f" reducer: `{result['baseline_reducer']}`"
                f" value: `{result['baseline_value']:.6f}`"
            ),
            f"- values: `{result['baseline_values']}`",
            "",
            "## Candidate",
            (
                f"- count: `{result['candidate_count']}`"
                f" reducer: `{result['candidate_reducer']}`"
                f" value: `{result['candidate_value']:.6f}`"
            ),
            f"- values: `{result['candidate_values']}`",
        ]
    )
    outlier_method = result.get("outlier_method")
    if outlier_method and outlier_method != "none":
        lines.extend(
            [
                "",
                "## Outliers",
                f"- method: `{outlier_method}`",
                f"- threshold: `{result['outlier_threshold']}`",
                f"- baseline: `{result['baseline_outliers']}`",
                f"- candidate: `{result['candidate_outliers']}`",
            ]
        )
    return "\n".join(lines) + "\n"


def maybe_write_outputs(args: argparse.Namespace, result: dict[str, object]) -> None:
    if args.out_json is not None:
        args.out_json.parent.mkdir(parents=True, exist_ok=True)
        args.out_json.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    if args.out_markdown is not None:
        args.out_markdown.parent.mkdir(parents=True, exist_ok=True)
        args.out_markdown.write_text(render_markdown(result))


def main() -> int:
    args = parse_args()
    direct_file_mode = bool(args.baseline_files or args.candidate_files)
    if direct_file_mode and args.manifest is not None:
        raise SystemExit("choose either direct metrics files or a manifest, not both")

    if direct_file_mode:
        if not args.baseline_files or not args.candidate_files:
            raise SystemExit(
                "direct file mode requires at least one --baseline-file and one --candidate-file"
            )
        if not args.metric:
            raise SystemExit("direct file mode requires --metric")
        if not args.goal:
            raise SystemExit("direct file mode requires --goal")
        metric = args.metric
        goal = args.goal
        baseline_values = extract_metric_values_from_files(args.baseline_files, metric)
        candidate_values = extract_metric_values_from_files(args.candidate_files, metric)
        mode = "direct-files"
        subject = {
            "baseline_files": [str(path) for path in args.baseline_files],
            "candidate_files": [str(path) for path in args.candidate_files],
        }
    else:
        if args.manifest is None:
            raise SystemExit(
                "provide either an experiment manifest or direct baseline/candidate files"
            )
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
        mode = "manifest"
        subject = {"manifest": str(args.manifest)}

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

    result = {
        "mode": mode,
        "subject": subject,
        "candidate_id": args.candidate_id,
        "metric": metric,
        "goal": goal,
        "baseline_count": len(baseline_values),
        "candidate_count": len(candidate_values),
        "baseline_reducer": args.baseline_reducer,
        "candidate_reducer": args.candidate_reducer,
        "baseline_value": baseline_summary,
        "candidate_value": candidate_summary,
        "baseline_values": baseline_values,
        "candidate_values": candidate_values,
        "improvement_pct": improvement,
        "outlier_method": args.outlier_method,
        "outlier_threshold": args.outlier_threshold,
        "baseline_outliers": baseline_outliers,
        "candidate_outliers": candidate_outliers,
    }
    maybe_write_outputs(args, result)

    if mode == "manifest":
        print(f"experiment: {manifest['experiment_id']}")
    else:
        print("experiment: <direct-files>")
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
