---
name: "sched-perf-collection"
description: "Use when scheduler-related performance evidence has to be collected reproducibly for a workload, target selector, or experiment phase. Covers low-overhead counters first, escalation to traces, and repo scripts that keep capture deterministic without baking a workflow into the host."
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
   - use `scripts/collect_perf.sh` when you want a deterministic wrapper around `perf stat` or `perf record`
   - use `scripts/collect_sched_timeline.sh` when the open question is scheduler ordering, wakeup chains, or migration churn and you want `perf sched record` plus derived `timehist` and `latency` artifacts
   - choose `--driver host` for ordinary collection and `--driver daemon` when attach-style privileged capture is required
   - direct shell capture is also fine when the wrapper would get in the way
   - when non-root collection is blocked by `perf_event_paranoid`, cgroup visibility, or attach permissions, prefer the structured `sched_ext_daemon` `collect_perf` action instead of inventing a root shell workaround
   - `perf stat`, `/proc/schedstat`, `/proc/<pid>/schedstat`, PSI, `pidstat -w`, `mpstat -P ALL`, and `vmstat`
   - collect just enough to answer whether the issue looks like queueing, migration, wakeup latency, or plain saturation
3. Escalate only when the summary leaves a scheduler-specific gap.
   - use `perf sched` when ordering, wakeup chains, or migration churn are the open question
   - when that timeline capture needs privilege, prefer the structured daemon `collect_sched` action or the `scripts/collect_sched_timeline.sh --driver daemon ...` wrapper instead of a root shell workaround
   - use `perf record` or focused BPF tracing only when the low-overhead view cannot distinguish competing hypotheses
4. Persist evidence as normal artifacts.
   - save the exact command line, raw csv or log output, and any reduced metrics in a stable workspace path
   - a conventional location such as `.nanoclaw/apps/sched-claw/artifacts/<label>/` is fine, but not required by the host
5. Keep each evidence bundle narrow.
   - one bundle should answer one question or one command family
   - split baseline and candidate captures rather than mixing them into one opaque blob

## Rules
- Do not invent a dedicated collection tool when shell commands and artifact capture already work.
- When privilege is required, use the daemon's structured perf capture surface rather than `sudo perf ...` or `sh -c`.
- Do not jump to high-overhead tracing before you have low-overhead evidence that justifies it.
- If the host is noisy, record that as a note instead of pretending the sample is clean.
- Prefer short, replayable commands over interactive sessions.
- Keep collection logic in scripts or shell commands, not in new host commands.

## Output checklist
- exact command or collector used
- target selector and phase
- artifact paths
- any parsed direct metrics
- one-sentence summary of what the capture is for

## Reference Material
- `references/collection-matrix.md`

## Optional Helper Scripts
- `scripts/collect_perf.sh`
  - wraps `perf stat` or `perf record`
  - supports both `--driver host` and `--driver daemon`
  - supports command, pid, uid, gid, and cgroup targets
  - writes the exact command line plus stdout and stderr artifacts
  - preferred for non-root or same-user collection flows
- `scripts/collect_sched_timeline.sh`
  - wraps `perf sched record` and renders `perf sched timehist` plus `perf sched latency`
  - supports both `--driver host` and `--driver daemon`
  - supports command, pid, uid, gid, and cgroup targets
  - writes raw `perf.sched.data` plus derived scheduler timeline artifacts
