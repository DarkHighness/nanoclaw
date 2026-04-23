#!/usr/bin/env python3
"""Summarize captured sched-ext build and verifier artifacts."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("artifact_dir", type=Path, help="Path created by the capture helper.")
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


def parse_key_value_file(path: Path) -> dict[str, str]:
    if not path.is_file():
        return {}
    values: dict[str, str] = {}
    for line in path.read_text().splitlines():
        if "=" not in line:
            continue
        key, raw = line.split("=", 1)
        values[key.strip()] = raw.strip()
    return values


def read_excerpt(path: Path, limit: int = 6) -> list[str]:
    if not path.is_file():
        return []
    lines = [line.rstrip() for line in path.read_text().splitlines() if line.strip()]
    return lines[:limit]


def classify_build(stderr_text: str) -> tuple[str, list[str]]:
    lowered = stderr_text.lower()
    if "command not found" in lowered:
        return ("missing-tool", ["install the missing command or fix the build script PATH"])
    if "no such file or directory" in lowered:
        return ("missing-input", ["check source, include, and build-script paths"])
    if "fatal error:" in lowered or "error:" in lowered:
        return ("compile-error", ["fix the compile error before re-running verifier checks"])
    return ("build-failed", ["inspect compiler stderr and command output"])


def classify_verify(text: str) -> tuple[str, list[str]]:
    lowered = text.lower()
    hints: list[str] = []
    if "verifier" in lowered:
        hints.append("inspect pointer, bounds, and control-flow assumptions")
    if "libbpf" in lowered:
        hints.append("check libbpf loader expectations and object layout")
    if "btf" in lowered or "/sys/kernel/btf/vmlinux" in lowered:
        hints.append("check BTF availability and CO-RE assumptions")
    if "unknown func" in lowered or "unknown helper" in lowered or "kfunc" in lowered:
        hints.append("verify helper or kfunc availability on the target kernel")
    if "struct_ops" in lowered:
        hints.append("check struct_ops layout and sched-ext ABI expectations")
    if not hints:
        hints.append("inspect verifier stdout and stderr for the first rejecting instruction")

    if "btf" in lowered or "/sys/kernel/btf/vmlinux" in lowered:
        category = "btf-or-core-mismatch"
    elif "unknown func" in lowered or "unknown helper" in lowered or "kfunc" in lowered:
        category = "helper-availability"
    elif "libbpf" in lowered:
        category = "libbpf-load-failure"
    elif "verifier" in lowered:
        category = "verifier-rejection"
    else:
        category = "verify-failed"
    return (category, hints)


def render_markdown(summary: dict[str, object]) -> str:
    lines = [
        f"# build and verifier summary: {summary['artifact_dir']}",
        "",
        "## Summary",
        f"- overall_status: `{summary['overall_status']}`",
        f"- source: `{summary['source']}`",
        f"- object: `{summary['object']}`",
        f"- build_status: `{summary['build_status']}`",
        f"- verify_status: `{summary['verify_status']}`",
        f"- classification: `{summary['classification']}`",
        "",
        "## Hints",
    ]
    lines.extend([f"- {hint}" for hint in summary["hints"]])
    if summary["build_excerpt"]:
        lines.extend(["", "## Build Excerpt"])
        lines.extend([f"- `{line}`" for line in summary["build_excerpt"]])
    if summary["verify_excerpt"]:
        lines.extend(["", "## Verify Excerpt"])
        lines.extend([f"- `{line}`" for line in summary["verify_excerpt"]])
    return "\n".join(lines) + "\n"


def maybe_write_outputs(args: argparse.Namespace, summary: dict[str, object]) -> None:
    if args.out_json is not None:
        args.out_json.parent.mkdir(parents=True, exist_ok=True)
        args.out_json.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    if args.out_markdown is not None:
        args.out_markdown.parent.mkdir(parents=True, exist_ok=True)
        args.out_markdown.write_text(render_markdown(summary))


def main() -> int:
    args = parse_args()
    summary_env = parse_key_value_file(args.artifact_dir / "summary.env")
    context = parse_key_value_file(args.artifact_dir / "context.txt")
    build_status = summary_env.get("build_status", "<missing>")
    verify_status = summary_env.get("verify_status", "<missing>")
    build_stderr = (args.artifact_dir / "build.stderr.log").read_text() if (
        args.artifact_dir / "build.stderr.log"
    ).is_file() else ""
    verify_stderr = (args.artifact_dir / "verify.stderr.log").read_text() if (
        args.artifact_dir / "verify.stderr.log"
    ).is_file() else ""
    verify_stdout = (args.artifact_dir / "verify.stdout.log").read_text() if (
        args.artifact_dir / "verify.stdout.log"
    ).is_file() else ""

    classification = "success"
    hints: list[str] = []
    overall_status = "success"
    if build_status != "0":
        overall_status = "failed"
        classification, hints = classify_build(build_stderr)
    elif verify_status not in {"<missing>", "0"}:
        overall_status = "failed"
        classification, hints = classify_verify(f"{verify_stdout}\n{verify_stderr}")
    elif verify_status == "<missing>":
        overall_status = "build-only"
        classification = "build-only"
        hints = ["no verifier command was captured for this artifact directory"]

    summary = {
        "artifact_dir": str(args.artifact_dir),
        "overall_status": overall_status,
        "source": context.get("source", "<unknown>"),
        "object": context.get("object", "<unknown>"),
        "build_status": build_status,
        "verify_status": verify_status,
        "classification": classification,
        "hints": hints,
        "build_excerpt": read_excerpt(args.artifact_dir / "build.stderr.log"),
        "verify_excerpt": read_excerpt(args.artifact_dir / "verify.stderr.log")
        or read_excerpt(args.artifact_dir / "verify.stdout.log"),
    }
    maybe_write_outputs(args, summary)

    print(f"artifact_dir: {args.artifact_dir}")
    print(f"overall_status: {overall_status}")
    print(f"classification: {classification}")
    print(f"source: {summary['source']}")
    print(f"object: {summary['object']}")
    print(f"build_status: {build_status}")
    print(f"verify_status: {verify_status}")
    for hint in hints:
        print(f"hint: {hint}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
