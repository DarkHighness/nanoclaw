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
This SOP guides the loop, but the host should not hard-code it. Use only the
steps that match the current evidence and question.

1. Define the failure contract before collecting anything.
   - State the workload entrypoint, the bad outcome, the affected CPUs or cgroups, and whether the bad phase is steady-state, bursty, or startup-only.
   - Record the success metric that will later decide whether the new scheduler beats CFS.
   - If the workload target is already known, persist it structurally as a script, pid, uid, gid, or cgroup target instead of leaving it only in prose.
2. Establish a reproducible baseline directory.
   - If the local host is running `sched-claw`, use the experiment substrate when it helps keep the workload contract and metric trail durable: `sched-claw experiment init ...`, `record-baseline ...`, `score ...`.
   - Unless the repository already has a stronger convention, create `.nanoclaw/apps/sched-claw/artifacts/<run-label>/`.
   - Save commands, raw outputs, and short notes side by side so the later sched-ext comparison can replay the same evidence path.
3. Capture low-overhead scheduler evidence first.
   - Prefer `uname -a`, `lscpu`, `/proc/schedstat`, `/proc/<pid>/schedstat`, `/proc/pressure/{cpu,io,memory}`, `mpstat -P ALL`, `pidstat -w`, `vmstat`, and `perf stat`.
   - Start with summaries that tell you whether the problem is queueing, migration, wakeup latency, or plain saturation.
4. Escalate to scheduler traces only when the summaries justify it.
   - Use `perf sched record`, `perf sched timehist`, `perf sched latency`, or focused tracing under `/sys/kernel/tracing` when you need wakeup chains, dispatch order, or migration churn.
   - Keep the trace window short and aligned to the bad phase.
5. Separate fact, inference, and unknown.
   - Fact: direct counters, trace events, latency distributions, queue lengths, PSI windows, per-CPU imbalance.
   - Inference: what those facts imply about wakeup placement, slice sizing, migration cost, preemption timing, or class interference.
   - Unknown: what still needs another phase, another workload slice, or another kernel signal.
6. Rank scheduler-specific hypotheses.
   - Distinguish scheduler pathologies from generic CPU, memory, or IO pressure.
   - If PSI or vmstat indicates the dominant bottleneck is not CPU scheduling, say so explicitly instead of forcing a scheduler conclusion.
7. End with a scheduler design implication.
   - State what the next `sched-ext` policy should optimize: locality, latency, fairness, throughput, tail control, or workload isolation.
   - Also state what the policy should explicitly avoid making worse.
   - If the next step needs concrete code scaffolding, inspect `sched-claw template list` or `sched-claw template show <name>` and let the active design skill decide which template, if any, to materialize.
   - If throughput or latency are not measurable on this workload, say that explicitly and record the proxy basis you are using instead, such as IPC or CPI.

## Artifact Checklist
- `baseline.md` or equivalent short note with workload, metric, and bad phase definition
- raw command outputs for the baseline counters
- trace capture command lines if deeper tracing was required
- a final hypothesis list ranked by confidence
- one paragraph that maps the evidence to the next sched-ext policy change
- if available, one `sched-claw experiment record-baseline ...` entry that captures the same metric set structurally

## Rules
- Do not invent a dedicated collection tool. Use normal shell commands and preserve artifacts.
- Prefer low-overhead counters and summaries before invasive tracing.
- If evidence is noisy or mixed, say so and lower confidence.
- Before comparing schedulers, capture one clean CFS baseline with the same workload and command set.
- When possible, store baseline metrics in the experiment manifest instead of only in Markdown notes.
- If the repository already has a reporting convention, follow it instead of forcing the default artifact path above.

## Reference Material
- `references/official-docs.md`
- `references/evidence-checklist.md`
