#!/usr/bin/env python3
"""Summarize perf sched latency output into durable rows and top offenders."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


FLOAT_RE = re.compile(r"^-?\d+(?:\.\d+)?$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--out-json", type=Path)
    parser.add_argument("--top", type=int, default=10)
    return parser.parse_args()


def parse_latency_line(line: str) -> dict[str, object] | None:
    if "|" not in line:
        return None
    columns = [part.strip() for part in line.split("|")]
    if len(columns) < 5:
        return None
    task = columns[0]
    if not task or task.lower().startswith("task"):
        return None
    numeric = columns[1:5]
    if not all(FLOAT_RE.match(value) for value in numeric):
        return None
    return {
        "task": task,
        "runtime_ms": float(columns[1]),
        "switches": int(float(columns[2])),
        "avg_delay_ms": float(columns[3]),
        "max_delay_ms": float(columns[4]),
    }


def parse_latency_file(path: Path) -> list[dict[str, object]]:
    rows: list[dict[str, object]] = []
    for raw_line in path.read_text(errors="replace").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        parsed = parse_latency_line(line)
        if parsed is not None:
            rows.append(parsed)
    return rows


def render_markdown(rows: list[dict[str, object]], top: int) -> str:
    top_rows = sorted(rows, key=lambda row: float(row["max_delay_ms"]), reverse=True)[:top]
    lines = [
        "# perf sched latency summary",
        "",
        f"- tasks parsed: `{len(rows)}`",
        f"- top offenders: `{len(top_rows)}`",
        "",
        "| task | runtime_ms | switches | avg_delay_ms | max_delay_ms |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for row in top_rows:
        lines.append(
            "| {task} | {runtime_ms:.3f} | {switches} | {avg_delay_ms:.3f} | {max_delay_ms:.3f} |".format(
                **row
            )
        )
    return "\n".join(lines) + "\n"


def main() -> int:
    args = parse_args()
    rows = parse_latency_file(args.input)
    payload = {
        "rows": rows,
        "top_offenders": sorted(rows, key=lambda row: float(row["max_delay_ms"]), reverse=True)[
            : args.top
        ],
    }
    if args.out_json:
        args.out_json.write_text(json.dumps(payload, indent=2) + "\n")
    rendered = render_markdown(rows, args.top)
    if args.output:
        args.output.write_text(rendered)
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
