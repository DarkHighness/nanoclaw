---
name: "sched-perf-collection"
description: "Use when scheduler-related performance evidence has to be collected reproducibly for a workload, target selector, or experiment phase. Covers low-overhead counters first, escalation to traces, and durable evidence records in the experiment manifest."
aliases:
  - "perf-collection"
tags:
  - "linux"
  - "scheduler"
  - "performance"
  - "collection"
---

# sched Performance Collection

## When to use
- A workload contract exists, but the evidence set is still missing.
- A baseline or candidate run needs supporting scheduler evidence beyond a single metrics file.
- The next question is "what should we measure?" rather than "what does this mean?"

## Read before acting
- `references/collection-matrix.md`

## Workflow
1. Start from the workload contract, not from the tool.
   - keep the target selector explicit: script, pid, uid, gid, or cgroup
   - decide whether the bad phase is startup, steady-state, burst, or shutdown
2. Prefer low-overhead summaries first.
   - start with the manifest-backed collection policy when `perf stat` is enough:
     - `sched-claw experiment set-collection-policy <experiment> --perf-stat-profile proxy_basic`
     - `sched-claw experiment run <experiment> ...`
   - `perf stat`, `/proc/schedstat`, `/proc/<pid>/schedstat`, PSI, `pidstat -w`, `mpstat -P ALL`, and `vmstat`
   - collect just enough to answer whether the issue looks like queueing, migration, wakeup latency, or plain saturation
3. Escalate only when the summary leaves a scheduler-specific gap.
   - use `perf sched` when ordering, wakeup chains, or migration churn are the open question
   - use `perf record` or focused BPF tracing only when the low-overhead view cannot distinguish competing hypotheses
4. Persist evidence as typed experiment records.
   - `sched-claw experiment run` already auto-records `perf_stat` evidence when the active collection policy enables it
   - `sched-claw experiment record-evidence <experiment> --evidence-id ... --kind ...`
   - include artifact paths, collector command, phase, scheduler, candidate id, and any directly measured metrics
5. Keep each evidence record narrow.
   - one record should answer one question or one command family
   - split baseline and candidate captures rather than mixing them into one opaque blob

## Rules
- Do not invent a dedicated collection tool when shell commands and artifact capture already work.
- Do not jump to high-overhead tracing before you have low-overhead evidence that justifies it.
- If the host is noisy, record that as a note instead of pretending the sample is clean.
- Prefer short, replayable commands over interactive sessions.

## Output checklist
- exact command or collector used
- target selector and phase
- artifact paths
- any parsed direct metrics
- one-sentence summary of what the capture is for

## Reference Material
- `references/collection-matrix.md`
