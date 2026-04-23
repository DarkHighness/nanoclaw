#!/usr/bin/env python3
"""Compose a durable perf evidence note from raw capture artifacts.

This helper stays in the skill layer so the agent can standardize evidence
notes without forcing a host-side workflow or scoring policy.
"""

from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--capture-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--title")
    parser.add_argument("--fact", action="append", default=[])
    parser.add_argument("--inference", action="append", default=[])
    parser.add_argument("--unknown", action="append", default=[])
    parser.add_argument("--recommendation", action="append", default=[])
    parser.add_argument("--out-json", type=Path)
    return parser.parse_args()


def parse_counter_value(raw: str) -> float | None:
    raw = raw.strip().replace(",", "")
    if not raw or "<not counted>" in raw or "<not supported>" in raw:
        return None
    try:
        return float(raw)
    except ValueError:
        return None


def parse_perf_stat(csv_path: Path) -> list[dict[str, object]]:
    if not csv_path.is_file():
        return []
    rows: list[dict[str, object]] = []
    with csv_path.open(newline="") as handle:
        reader = csv.reader(handle)
        for row in reader:
            if len(row) < 3:
                continue
            value = parse_counter_value(row[0])
            if value is None:
                continue
            rows.append(
                {
                    "value": value,
                    "unit": row[1].strip(),
                    "event": row[2].strip(),
                }
            )
    return rows


def read_json(path: Path) -> object | None:
    if not path.is_file():
        return None
    return json.loads(path.read_text())


def render_selector(selector_doc: object | None) -> str:
    if not isinstance(selector_doc, dict):
        return "<unknown>"
    selector = selector_doc.get("selector")
    resolved = selector_doc.get("resolved_pids")
    selector_text = json.dumps(selector, ensure_ascii=False) if selector is not None else "<unknown>"
    if isinstance(resolved, list) and resolved:
        selector_text += f" | resolved_pids={','.join(str(value) for value in resolved)}"
    return selector_text


def render_command(command_doc: object | None) -> str:
    if not isinstance(command_doc, list):
        return "<unknown>"
    return " ".join(str(part) for part in command_doc)


def list_or_placeholder(values: list[str], placeholder: str) -> list[str]:
    return values if values else [placeholder]


def main() -> int:
    args = parse_args()
    capture_dir = args.capture_dir
    if not capture_dir.is_dir():
        raise SystemExit(f"capture dir does not exist: {capture_dir}")

    command_path = capture_dir / "perf.command.json"
    selector_path = capture_dir / "perf.selector.json"
    stat_path = capture_dir / "perf.stat.csv"
    data_path = capture_dir / "perf.data"
    report_path = capture_dir / "perf.report.txt"
    script_path = capture_dir / "perf.script.txt"
    stdout_path = capture_dir / "perf.stdout.log"
    stderr_path = capture_dir / "perf.stderr.log"

    command_doc = read_json(command_path)
    selector_doc = read_json(selector_path)
    counters = parse_perf_stat(stat_path)

    mode = "record" if data_path.is_file() else "stat" if stat_path.is_file() else "unknown"
    title = args.title or f"perf evidence: {capture_dir.name}"

    artifacts = [
        path
        for path in [
            command_path,
            selector_path,
            stat_path,
            data_path,
            report_path,
            script_path,
            stdout_path,
            stderr_path,
        ]
        if path.exists()
    ]

    summary = {
        "title": title,
        "capture_dir": str(capture_dir),
        "mode": mode,
        "selector": selector_doc,
        "command": command_doc,
        "counters": counters,
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
        f"- mode: `{mode}`",
        f"- selector: `{render_selector(selector_doc)}`",
        f"- command: `{render_command(command_doc)}`",
        "",
        "## Direct Facts",
    ]
    if counters:
        for row in counters:
            lines.append(
                "- `{event}` = `{value}` {unit}".format(
                    event=row["event"],
                    value=row["value"],
                    unit=row["unit"] or "",
                ).rstrip()
            )
    else:
        lines.append("- `<no perf.stat.csv counters detected>`")

    lines.extend(
        [
            "",
            "## Analyst Facts",
        ]
    )
    lines.extend(f"- {item}" for item in list_or_placeholder(args.fact, "<fill in factual findings>"))
    lines.extend(
        [
            "",
            "## Inferences",
        ]
    )
    lines.extend(
        f"- {item}"
        for item in list_or_placeholder(args.inference, "<fill in scheduler implications>")
    )
    lines.extend(
        [
            "",
            "## Unknowns",
        ]
    )
    lines.extend(f"- {item}" for item in list_or_placeholder(args.unknown, "<fill in open questions>"))
    lines.extend(
        [
            "",
            "## Recommendations",
        ]
    )
    lines.extend(
        f"- {item}"
        for item in list_or_placeholder(
            args.recommendation, "<fill in next capture, code change, or rollback gate>"
        )
    )
    lines.extend(
        [
            "",
            "## Artifacts",
        ]
    )
    lines.extend(f"- `{path}`" for path in artifacts)
    lines.append("")

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(lines))
    if args.out_json:
        args.out_json.parent.mkdir(parents=True, exist_ok=True)
        args.out_json.write_text(json.dumps(summary, indent=2) + "\n")

    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
