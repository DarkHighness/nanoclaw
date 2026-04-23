#!/usr/bin/env python3
"""Validate a sched-claw workload contract TOML file."""

from __future__ import annotations

import argparse
import json
import tomllib
from pathlib import Path

ALLOWED_SELECTOR_KINDS = {"script", "pid", "uid", "gid", "cgroup"}
ALLOWED_GOALS = {"minimize", "maximize"}
ALLOWED_BASES = {"direct", "proxy_estimate"}
COMMON_PROXY_METRICS = {"ipc", "cpi", "branch_miss_rate", "cache_miss_rate"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("contract", type=Path, help="Path to a contract TOML file.")
    parser.add_argument(
        "--out-json",
        type=Path,
        help="Optional path to write a JSON validation summary.",
    )
    parser.add_argument(
        "--out-markdown",
        type=Path,
        help="Optional path to write a Markdown validation summary.",
    )
    return parser.parse_args()


def load_contract(path: Path) -> dict:
    return tomllib.loads(path.read_text())


def ensure_string_list(value: object, field_name: str, errors: list[str]) -> list[str]:
    if value is None:
        return []
    if not isinstance(value, list):
        errors.append(f"{field_name} must be a list of strings")
        return []
    parsed: list[str] = []
    for item in value:
        if isinstance(item, str) and item.strip():
            parsed.append(item.strip())
        else:
            errors.append(f"{field_name} entries must be non-empty strings")
            return []
    return parsed


def render_markdown(summary: dict[str, object]) -> str:
    lines = [
        f"# workload contract validation: {summary['path']}",
        "",
        "## Summary",
        f"- status: `{summary['status']}`",
        f"- name: `{summary['name']}`",
        f"- selector: `{summary['selector_kind']}:{summary['selector_value']}`",
        f"- primary metric: `{summary['primary_metric']}`",
        f"- primary goal: `{summary['primary_goal']}`",
        f"- basis: `{summary['performance_basis']}`",
        f"- guardrails: `{summary['guardrails']}`",
        f"- proxy metrics: `{summary['proxy_metrics']}`",
    ]
    if summary["errors"]:
        lines.extend(["", "## Errors"])
        lines.extend([f"- {item}" for item in summary["errors"]])
    if summary["warnings"]:
        lines.extend(["", "## Warnings"])
        lines.extend([f"- {item}" for item in summary["warnings"]])
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
    contract = load_contract(args.contract)
    errors: list[str] = []
    warnings: list[str] = []

    name = contract.get("name")
    if not isinstance(name, str) or not name.strip():
        errors.append("name must be a non-empty string")
        name = "<missing>"

    selector_kind = contract.get("selector_kind")
    selector_value = contract.get("selector_value")
    if not isinstance(selector_kind, str) or selector_kind not in ALLOWED_SELECTOR_KINDS:
        errors.append("selector_kind must be one of script|pid|uid|gid|cgroup")
        selector_kind = "<invalid>"
    if not isinstance(selector_value, str) or not selector_value.strip():
        errors.append("selector_value must be a non-empty string")
        selector_value = "<missing>"

    primary_metric = contract.get("primary_metric")
    if not isinstance(primary_metric, str) or not primary_metric.strip():
        errors.append("primary_metric must be a non-empty string")
        primary_metric = "<missing>"

    primary_goal = contract.get("primary_goal")
    if not isinstance(primary_goal, str) or primary_goal not in ALLOWED_GOALS:
        errors.append("primary_goal must be one of minimize|maximize")
        primary_goal = "<invalid>"

    performance_basis = contract.get("performance_basis")
    if not isinstance(performance_basis, str) or performance_basis not in ALLOWED_BASES:
        errors.append("performance_basis must be direct or proxy_estimate")
        performance_basis = "<invalid>"

    guardrails = ensure_string_list(contract.get("guardrails"), "guardrails", errors)
    proxy_metrics = ensure_string_list(contract.get("proxy_metrics"), "proxy_metrics", errors)

    if performance_basis == "proxy_estimate" and not proxy_metrics:
        errors.append("proxy_estimate contracts must declare at least one proxy_metrics entry")

    if performance_basis == "direct" and isinstance(primary_metric, str):
        if primary_metric in COMMON_PROXY_METRICS:
            warnings.append(
                "primary_metric looks like a proxy metric while performance_basis is direct"
            )
    if performance_basis == "direct" and proxy_metrics:
        warnings.append(
            "proxy_metrics are present even though performance_basis is direct; make sure they are hints, not the decision basis"
        )

    summary = {
        "path": str(args.contract),
        "status": "valid" if not errors else "invalid",
        "name": name,
        "selector_kind": selector_kind,
        "selector_value": selector_value,
        "primary_metric": primary_metric,
        "primary_goal": primary_goal,
        "performance_basis": performance_basis,
        "guardrails": guardrails,
        "proxy_metrics": proxy_metrics,
        "warnings": warnings,
        "errors": errors,
    }
    maybe_write_outputs(args, summary)

    print(f"contract: {args.contract}")
    print(f"status: {summary['status']}")
    print(f"name: {name}")
    print(f"selector: {selector_kind}:{selector_value}")
    print(f"primary_metric: {primary_metric}")
    print(f"primary_goal: {primary_goal}")
    print(f"performance_basis: {performance_basis}")
    print(f"guardrails: {guardrails}")
    print(f"proxy_metrics: {proxy_metrics}")
    for warning in warnings:
        print(f"warning: {warning}")
    for error in errors:
        print(f"error: {error}")
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
