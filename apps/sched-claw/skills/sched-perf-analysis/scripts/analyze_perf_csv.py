#!/usr/bin/env python3
"""Summarize perf stat CSV files and optionally render a chart.

This helper lives in the skill layer so the agent can pick the right reducer,
metrics, and artifact paths without inheriting a fixed host-side analysis flow.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import statistics
from pathlib import Path

try:
    import polars as pl
except Exception:  # pragma: no cover - optional at runtime
    pl = None

try:
    import pandas as pd
except Exception:  # pragma: no cover - optional at runtime
    pd = None

try:
    import matplotlib.pyplot as plt
except Exception:  # pragma: no cover - optional at runtime
    plt = None


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("csv_files", nargs="+", type=Path)
    parser.add_argument("--metric", action="append", dest="metrics")
    parser.add_argument(
        "--reducer",
        choices=("median", "mean", "min", "max"),
        default="median",
    )
    parser.add_argument("--out-json", type=Path)
    parser.add_argument("--out-env", type=Path)
    parser.add_argument("--out-markdown", type=Path)
    parser.add_argument("--plot", type=Path)
    parser.add_argument("--title", default="sched-claw perf summary")
    parser.add_argument("--derive-proxies", action="store_true")
    return parser.parse_args()


def parse_counter_value(raw: str) -> float | None:
    value = raw.strip().replace(",", "")
    if not value or "<not counted>" in value or "<not supported>" in value:
        return None
    try:
        parsed = float(value)
    except ValueError:
        return None
    return parsed if math.isfinite(parsed) else None


def parse_perf_csv(path: Path) -> list[dict[str, object]]:
    rows: list[dict[str, object]] = []
    with path.open(newline="") as handle:
        reader = csv.reader(handle)
        for row in reader:
            if len(row) < 3:
                continue
            value = parse_counter_value(row[0])
            if value is None:
                continue
            unit = row[1].strip()
            event = row[2].strip()
            if not event:
                continue
            rows.append(
                {
                    "source": path.name,
                    "event": event,
                    "unit": unit,
                    "value": value,
                }
            )
    return rows


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


def summarize_with_polars(rows: list[dict[str, object]], reducer: str) -> list[dict[str, object]]:
    if pl is None:
        raise RuntimeError("polars is unavailable")
    frame = pl.DataFrame(rows)
    summary = frame.group_by(["source", "event", "unit"]).agg(
        [
            pl.len().alias("count"),
            pl.col("value").min().alias("min"),
            pl.col("value").max().alias("max"),
            getattr(pl.col("value"), reducer)().alias("summary"),
        ]
    )
    return summary.sort(["event", "source"]).to_dicts()


def summarize_with_python(rows: list[dict[str, object]], reducer: str) -> list[dict[str, object]]:
    buckets: dict[tuple[str, str, str], list[float]] = {}
    for row in rows:
        key = (str(row["source"]), str(row["event"]), str(row["unit"]))
        buckets.setdefault(key, []).append(float(row["value"]))
    summary: list[dict[str, object]] = []
    for (source, event, unit), values in sorted(buckets.items(), key=lambda item: (item[0][1], item[0][0])):
        summary.append(
            {
                "source": source,
                "event": event,
                "unit": unit,
                "count": len(values),
                "min": min(values),
                "max": max(values),
                "summary": reduce_values(values, reducer),
            }
        )
    return summary


def reduced_events_by_source(summary: list[dict[str, object]]) -> dict[str, dict[str, float]]:
    reduced: dict[str, dict[str, float]] = {}
    for row in summary:
        source = str(row["source"])
        event = str(row["event"])
        reduced.setdefault(source, {})[event] = float(row["summary"])
    return reduced


def divide(numerator: float | None, denominator: float | None) -> float | None:
    if numerator is None or denominator in (None, 0.0):
        return None
    value = numerator / denominator
    return value if math.isfinite(value) else None


def derive_proxy_metrics(summary: list[dict[str, object]]) -> list[dict[str, object]]:
    derived_rows: list[dict[str, object]] = []
    for source, events in sorted(reduced_events_by_source(summary).items()):
        instructions = events.get("instructions")
        cycles = events.get("cycles")
        branches = events.get("branches")
        branch_misses = events.get("branch-misses")
        cache_refs = events.get("cache-references")
        cache_misses = events.get("cache-misses")
        source_metrics = {
            "ipc": divide(instructions, cycles),
            "cpi": divide(cycles, instructions),
            "branch_miss_rate": divide(branch_misses, branches),
            "cache_miss_rate": divide(cache_misses, cache_refs),
        }
        for metric, value in source_metrics.items():
            if value is None:
                continue
            derived_rows.append(
                {
                    "source": source,
                    "metric": metric,
                    "value": value,
                }
            )
    return derived_rows


def write_markdown(path: Path, summary: list[dict[str, object]], reducer: str) -> None:
    write_markdown_with_proxies(path, summary, reducer, [])


def write_markdown_with_proxies(
    path: Path,
    summary: list[dict[str, object]],
    reducer: str,
    derived: list[dict[str, object]],
) -> None:
    lines = [
        f"# perf summary ({reducer})",
        "",
        "| source | event | unit | count | summary | min | max |",
        "| --- | --- | --- | ---: | ---: | ---: | ---: |",
    ]
    for row in summary:
        lines.append(
            "| {source} | {event} | {unit} | {count} | {summary:.6f} | {min:.6f} | {max:.6f} |".format(
                **row
            )
        )
    if derived:
        lines.extend(
            [
                "",
                "## Derived Proxy Metrics",
                "",
                "| source | metric | value |",
                "| --- | --- | ---: |",
            ]
        )
        for row in derived:
            lines.append(
                "| {source} | {metric} | {value:.6f} |".format(
                    **row
                )
            )
    path.write_text("\n".join(lines) + "\n")


def sanitize_env_key(value: str) -> str:
    sanitized = [
        char.upper() if char.isalnum() else "_"
        for char in value
    ]
    text = "".join(sanitized).strip("_")
    return text or "SOURCE"


def write_env(path: Path, derived: list[dict[str, object]]) -> None:
    grouped: dict[str, list[dict[str, object]]] = {}
    for row in derived:
        grouped.setdefault(str(row["source"]), []).append(row)
    lines: list[str] = []
    multi_source = len(grouped) > 1
    for source, rows in sorted(grouped.items()):
        prefix = f"{sanitize_env_key(source)}__" if multi_source else ""
        for row in rows:
            key = f"{prefix}{str(row['metric']).upper()}"
            lines.append(f"{key}={float(row['value']):.10f}")
    path.write_text("\n".join(lines) + ("\n" if lines else ""))


def write_plot(path: Path, summary: list[dict[str, object]], title: str) -> None:
    if plt is None:
        raise RuntimeError("matplotlib is unavailable")
    if pd is not None:
        frame = pd.DataFrame(summary)
        labels = frame.apply(lambda row: f"{row['source']}:{row['event']}", axis=1).tolist()
        values = frame["summary"].tolist()
    else:
        labels = [f"{row['source']}:{row['event']}" for row in summary]
        values = [float(row["summary"]) for row in summary]
    fig, ax = plt.subplots(figsize=(max(8, len(labels) * 0.9), 4.5))
    ax.bar(labels, values)
    ax.set_title(title)
    ax.set_ylabel("summary")
    ax.tick_params(axis="x", labelrotation=45)
    fig.tight_layout()
    fig.savefig(path)


def main() -> int:
    args = parse_args()
    rows = []
    for path in args.csv_files:
        rows.extend(parse_perf_csv(path))

    if args.metrics:
        wanted = set(args.metrics)
        rows = [row for row in rows if row["event"] in wanted]

    if not rows:
        raise SystemExit("no usable perf rows found")

    if pl is not None:
        summary = summarize_with_polars(rows, args.reducer)
    else:
        summary = summarize_with_python(rows, args.reducer)
    derived = derive_proxy_metrics(summary) if args.derive_proxies else []

    if args.out_json:
        payload: object = (
            {"summary": summary, "derived_proxies": derived}
            if args.derive_proxies
            else summary
        )
        args.out_json.write_text(json.dumps(payload, indent=2) + "\n")
    if args.out_env:
        write_env(args.out_env, derived)
    if args.out_markdown:
        write_markdown_with_proxies(args.out_markdown, summary, args.reducer, derived)
    if args.plot:
        write_plot(args.plot, summary, args.title)

    payload = (
        {"summary": summary, "derived_proxies": derived}
        if args.derive_proxies
        else summary
    )
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
