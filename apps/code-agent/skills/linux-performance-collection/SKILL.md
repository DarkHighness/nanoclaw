---
name: "linux-performance-collection"
description: "Use when a task requires collecting Linux performance evidence with perf, sysstat, or eBPF on a live host or benchmark run. Prefer low-overhead counters and first-pass triage before invasive tracing. Applicable to CPU, scheduler, memory, block I/O, and network collection. Do not trigger for pure code changes unless system evidence collection is part of the task."
---

# Linux Performance Collection

## When to use
- Collect first-pass Linux performance evidence on a host, VM, container, or benchmark target.
- Build a reproducible capture set before deeper analysis.
- Decide whether `perf`, `sysstat`, or eBPF should be used next.

## Read before collecting
- `references/collection-checklist.md` for the first-pass command set and USE-oriented coverage.
- `references/tool-recipes.md` for targeted `perf`, `sysstat`, and eBPF collection patterns.

## Workflow
1. Frame the capture before touching tools.
   - Record the problem statement, affected workload, time window, target PID/container/cgroup, kernel version, CPU model, and whether the run is production, staging, or benchmark-only.
   - Note whether the workload is latency-sensitive, throughput-oriented, periodic, or bursty.
2. Start with low-overhead counters and scheduler/device views.
   - Use the checklist in `references/collection-checklist.md`.
   - Prefer counters and summaries before high-rate sampling or broad tracing.
3. Add targeted `perf` collection only after the first pass.
   - Use `perf stat` for counters and IPC/CPI context.
   - Use `perf record` only when stack attribution is needed.
   - Pin the workload or scope the target if noisy neighbors would corrupt results.
4. Use eBPF only to fill a visibility gap.
   - Prefer existing `bpftrace` or BCC tools before writing a custom probe.
   - Use eBPF when you need latency distributions, per-event attribution, or off-CPU visibility that counters alone cannot provide.
5. Preserve evidence in a form that can be analyzed later.
   - Save exact commands, start/stop timestamps, stdout/stderr, and artifact paths.
   - Keep the workload phase and capture interval next to every artifact.
6. Record collection limits.
   - Note missing privileges, unavailable PMU events, disabled BTF, container restrictions, and any overhead risk that may bias the data.

## Collection rules
- Avoid changing the workload while measuring unless the goal is active benchmarking.
- Match sampling duration to workload periodicity; short captures hide burst behavior.
- Treat production tracing budgets conservatively. Narrow scope before raising frequency.
- If the host is containerized, capture both host-level and cgroup/process-level context.
- If the problem is intermittent, prefer repeated low-overhead snapshots over a single invasive run.

## Output expectations
- Produce an evidence inventory, not just raw command output.
- Group artifacts by CPU, scheduler, memory, block I/O, and network.
- Call out which gaps still require analysis or deeper tracing.
