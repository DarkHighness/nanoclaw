---
name: "sched-perf-analysis"
description: "Use when collected scheduler evidence needs to be turned into explicit facts, inferences, unknowns, and candidate design implications. Covers repo helper scripts for uv-managed analysis environments, pandas or polars summaries, and matplotlib plots."
aliases:
  - "perf-analysis"
tags:
  - "linux"
  - "scheduler"
  - "performance"
  - "analysis"
---

# sched Performance Analysis

## When to use
- Evidence has been collected and now needs interpretation.
- There are competing hypotheses about scheduler behavior.
- The next step depends on whether the evidence supports a code change, a new run, or a rollback.

## Read before acting
- `references/analysis-patterns.md`

## Workflow
1. Group evidence by question, not by command.
   - for example: wakeup placement, migration churn, starvation, queue buildup, or non-scheduler bottlenecks
2. Separate fact, inference, and unknown explicitly.
   - facts come from counters, traces, logs, and metrics
   - inferences explain what those facts imply about scheduler behavior
   - unknowns state what still blocks a confident conclusion
   - when IPC, CPI, or top-down counters are involved, state explicitly that
     they are proxy indicators unless the workload contract says otherwise
3. Check whether the workload contract still matches the evidence.
   - if the collected phase or selector was wrong, say so instead of over-interpreting the data
4. Use scriptable analysis helpers when the raw files are too wide to inspect directly.
   - `scripts/bootstrap_uv_env.sh` creates a uv-managed Python environment
   - `scripts/analyze_perf_csv.py` reduces one or more `perf stat` CSV captures and can emit JSON, Markdown, or a plot
   - `scripts/compose_perf_evidence.py` turns a raw perf capture directory into a durable Markdown or JSON evidence note
   - `scripts/render_perf_report.sh` turns `perf.data` into `perf report --stdio` and optional `perf script` artifacts
   - `scripts/summarize_metrics.py` remains useful for `metrics.env` style files
5. Persist the conclusion as normal artifacts or notes.
   - include the evidence paths, facts, inferences, unknowns, recommendations, and confidence
   - `scripts/compose_perf_evidence.py` is a good default when you want a durable evidence note without inventing a one-off Markdown shape
   - keep the reduction method explicit instead of assuming a host-provided scorer is authoritative
6. End with a decision surface.
   - is the next step another capture, a sched-ext code change, or a rollout stop?

## Rules
- Do not collapse direct evidence and inference into one sentence.
- Do not force a scheduler explanation when PSI, stalls, or hotspots point somewhere else.
- Lower confidence when the evidence is sparse, noisy, or contradictory.

## Output checklist
- evidence ids used
- factual findings
- inferred scheduler implication
- unknowns or missing data
- confidence level
- next action

## Reference Material
- `references/analysis-patterns.md`

## Optional Helper Scripts
- `scripts/bootstrap_uv_env.sh`
  - provisions a uv-managed Python environment for analysis helpers
- `scripts/analyze_perf_csv.py`
  - summarizes one or more `perf stat` CSV captures
  - can emit JSON, Markdown, and a matplotlib chart
- `scripts/compose_perf_evidence.py`
  - converts a raw perf capture directory into a durable Markdown or JSON evidence note
  - keeps facts, inferences, unknowns, recommendations, and artifact paths explicit
- `scripts/render_perf_report.sh`
  - renders `perf.data` into `perf report --stdio`
  - can also emit `perf script` output for deeper call-chain inspection
- `scripts/summarize_metrics.py`
  - summarizes one or more `metrics.env` files
  - supports caller-selected reducers instead of enforcing a fixed host policy
