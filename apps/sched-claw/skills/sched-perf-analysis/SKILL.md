---
name: "sched-perf-analysis"
description: "Use when collected scheduler evidence needs to be turned into explicit facts, inferences, unknowns, and candidate design implications. Covers durable analysis records instead of burying conclusions inside transcript prose."
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
4. Persist the conclusion.
   - `sched-claw experiment record-analysis <experiment> --analysis-id ...`
   - include evidence ids, facts, inferences, unknowns, recommendations, and confidence
   - if you need quick aggregation across many `metrics.env` files before writing the analysis, use `scripts/summarize_metrics.py` as a helper instead of baking one reduction strategy into the host
5. End with a decision surface.
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

## Optional Helper Script
- `scripts/summarize_metrics.py`
  - summarizes one or more `metrics.env` files
  - supports caller-selected reducers instead of enforcing a fixed host policy
