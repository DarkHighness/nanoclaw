#!/usr/bin/env python3
"""Compose a durable scheduler trace evidence note from perf sched artifacts."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


FLOAT_RE = re.compile(r"^-?\d+(?:\.\d+)?$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--capture-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--out-json", type=Path)
    parser.add_argument("--title")
    parser.add_argument("--fact", action="append", default=[])
    parser.add_argument("--inference", action="append", default=[])
    parser.add_argument("--unknown", action="append", default=[])
    parser.add_argument("--recommendation", action="append", default=[])
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
    if not path.is_file():
        return []
    rows: list[dict[str, object]] = []
    for raw_line in path.read_text(errors="replace").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        parsed = parse_latency_line(line)
        if parsed is not None:
            rows.append(parsed)
    return rows


def extract_timehist_excerpt(path: Path, limit: int) -> list[str]:
    if not path.is_file():
        return []
    lines: list[str] = []
    for raw_line in path.read_text(errors="replace").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        lines.append(line)
        if len(lines) >= limit:
            break
    return lines


def list_or_placeholder(values: list[str], placeholder: str) -> list[str]:
    return values if values else [placeholder]


def main() -> int:
    args = parse_args()
    capture_dir = args.capture_dir
    if not capture_dir.is_dir():
        raise SystemExit(f"capture dir does not exist: {capture_dir}")

    latency_path = capture_dir / "perf.sched.latency.txt"
    timehist_path = capture_dir / "perf.sched.timehist.txt"
    data_path = capture_dir / "perf.sched.data"
    selector_path = capture_dir / "perf.sched.selector.json"
    record_command_path = capture_dir / "perf.sched.record.command.json"
    timehist_command_path = capture_dir / "perf.sched.timehist.command.json"
    latency_command_path = capture_dir / "perf.sched.latency.command.json"

    latency_rows = parse_latency_file(latency_path)
    top_offenders = sorted(
        latency_rows,
        key=lambda row: float(row["max_delay_ms"]),
        reverse=True,
    )[: args.top]
    timehist_excerpt = extract_timehist_excerpt(timehist_path, args.top)
    title = args.title or f"scheduler trace evidence: {capture_dir.name}"

    artifacts = [
        path
        for path in [
            data_path,
            selector_path,
            record_command_path,
            timehist_command_path,
            latency_command_path,
            timehist_path,
            latency_path,
        ]
        if path.exists()
    ]

    payload = {
        "title": title,
        "capture_dir": str(capture_dir),
        "top_offenders": top_offenders,
        "timehist_excerpt": timehist_excerpt,
        "facts": args.fact,
        "inferences": args.inference,
        "unknowns": args.unknown,
        "recommendations": args.recommendation,
        "artifacts": [str(path) for path in artifacts],
    }

    lines = [
        f"# {title}",
        "",
        "## Capture",
        f"- dir: `{capture_dir}`",
        f"- data: `{data_path if data_path.exists() else '<missing>'}`",
        "",
        "## Top Delayed Tasks",
    ]
    if top_offenders:
        for row in top_offenders:
            lines.append(
                "- `{task}` avg_delay=`{avg_delay_ms:.3f}ms` max_delay=`{max_delay_ms:.3f}ms` switches=`{switches}` runtime=`{runtime_ms:.3f}ms`".format(
                    **row
                )
            )
    else:
        lines.append("- `<no parseable perf.sched.latency.txt rows detected>`")

    lines.extend(["", "## Timehist Excerpt"])
    if timehist_excerpt:
        lines.extend(f"- {line}" for line in timehist_excerpt)
    else:
        lines.append("- `<no perf.sched.timehist.txt excerpt detected>`")

    lines.extend(["", "## Analyst Facts"])
    lines.extend(f"- {item}" for item in list_or_placeholder(args.fact, "<fill in factual findings>"))
    lines.extend(["", "## Inferences"])
    lines.extend(
        f"- {item}"
        for item in list_or_placeholder(args.inference, "<fill in scheduler implications>")
    )
    lines.extend(["", "## Unknowns"])
    lines.extend(f"- {item}" for item in list_or_placeholder(args.unknown, "<fill in open questions>"))
    lines.extend(["", "## Recommendations"])
    lines.extend(
        f"- {item}"
        for item in list_or_placeholder(
            args.recommendation, "<fill in next capture, code change, or rollback gate>"
        )
    )
    lines.extend(["", "## Artifacts"])
    lines.extend(f"- `{path}`" for path in payload["artifacts"])

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(lines) + "\n")
    if args.out_json:
        args.out_json.write_text(json.dumps(payload, indent=2) + "\n")
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
