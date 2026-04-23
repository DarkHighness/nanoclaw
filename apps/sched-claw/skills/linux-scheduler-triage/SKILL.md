---
name: "linux-scheduler-triage"
description: "Use when a task requires collecting or interpreting Linux scheduling evidence before changing policy. Covers first-pass scheduler triage, evidence hygiene, and translating traces or counters into scheduler hypotheses."
aliases:
  - "scheduler-triage"
tags:
  - "linux"
  - "scheduler"
  - "performance"
---

# Linux Scheduler Triage

## When to use
- Diagnose latency, throughput collapse, jitter, or unfairness that may be scheduler-related.
- Build a baseline before writing or tuning a `sched-ext` scheduler.
- Turn raw `perf`, `schedstat`, PSI, or run-queue evidence into ranked scheduler hypotheses.

## Role
This is a composition skill, not a monolithic workflow. It should mostly decide
which lower-level skill to load next and what evidence question is still open.

## Compose with
- `sched-workload-contract`
  - when selector, direct metrics, or proxy basis are still vague
- `sched-perf-collection`
  - when evidence is still missing
- `sched-perf-analysis`
  - when raw artifacts exist but facts or inferences are not explicit yet
- `sched-policy-mapping`
  - when the next step is to turn triage output into concrete scheduler levers

## Triage responsibilities
1. Define the failure surface.
   - bad phase, affected selector, and success criteria
2. Narrow the evidence question.
   - queueing, migration, wakeup placement, starvation, or non-scheduler pressure
3. Decide the lightest next collector.
   - counters first, traces only when needed
4. Distinguish scheduler evidence from non-scheduler bottlenecks.
5. End with ranked hypotheses and the next skill to compose.

## Default helper choices
- `../sched-perf-collection/scripts/collect_sched_timeline.sh`
  - when scheduler ordering or migration churn is the missing evidence
- `../sched-perf-analysis/scripts/compose_perf_evidence.py`
  - when a raw perf bundle already exists and needs a durable note
- `../sched-perf-analysis/scripts/summarize_sched_latency.py`
  - when `perf sched latency` is the key artifact
- `../sched-perf-analysis/scripts/compose_sched_trace_evidence.py`
  - when the whole scheduler trace bundle should become one evidence note

## Artifact Checklist
- `baseline.md` or equivalent short note with workload, metric, and bad phase definition
- raw command outputs for the baseline counters
- trace capture command lines if deeper tracing was required
- a final hypothesis list ranked by confidence
- one paragraph that maps the evidence to the next sched-ext policy change
- optional structured sidecar files such as `facts.json`, `analysis.md`, or `baseline.env`

## Rules
- Do not invent a dedicated collection tool. Use normal shell commands and preserve artifacts.
- Prefer low-overhead counters and summaries before invasive tracing.
- If evidence is noisy or mixed, say so and lower confidence.
- Before comparing schedulers, capture one clean CFS baseline with the same workload and command set.
- If the repository already has a reporting convention, follow it instead of forcing the default artifact path above.

## Reference Material
- `references/official-docs.md`
- `references/evidence-checklist.md`
