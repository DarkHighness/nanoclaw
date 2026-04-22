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
- Turn raw `perf`, `schedstat`, PSI, or run-queue evidence into a ranked set of scheduling hypotheses.

## Workflow
1. Define the target symptom before collecting anything.
   - State the workload, bad outcome, affected CPUs or cgroups, and whether the run is steady-state or transient.
2. Collect baseline evidence with existing shell tools.
   - Prefer `uname -a`, `/proc/schedstat`, `/proc/pressure/*`, `mpstat -P ALL`, `pidstat -w`, `vmstat`, `perf stat`, and `perf sched`.
   - Save exact commands and outputs so later comparisons against the new scheduler are reproducible.
3. Separate scheduler evidence from generic CPU saturation.
   - High CPU utilization alone is not enough.
   - Look for queue growth, migration churn, wakeup delays, context-switch patterns, or load imbalance.
4. Build hypotheses from facts.
   - Fact: observed counters, traces, latency distributions, queue lengths.
   - Inference: what those observations imply about wakeup placement, slice size, migration cost, or fairness.
   - Unknown: what still needs a deeper trace or another workload phase.
5. End with a scheduler design implication.
   - State what the evidence suggests the next `sched-ext` policy should optimize: locality, latency, fairness, throughput, tail control, or cgroup isolation.

## Rules
- Do not invent a dedicated collection tool. Use normal shell commands and preserve artifacts.
- Prefer low-overhead counters and summaries before invasive tracing.
- If evidence is noisy or mixed, say so and lower confidence.
- Before comparing schedulers, capture one clean CFS baseline with the same workload and command set.
