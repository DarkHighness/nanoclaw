# Collection Matrix

Use the lightest collector that can falsify the current hypothesis.

## `perf_stat`
- Questions:
  - Is retiring efficiency low?
  - Are cycles or stalled frontend/backend dominating?
  - Did IPC or CPI move in the expected direction?
- Preferred path:
  - run `scripts/collect_perf.sh --mode stat ...` or an equivalent explicit
    `perf stat` command
  - persist the resulting `perf.stat.csv`, command line, and notes next to the
    workload artifacts
  - if an operator is intentionally using the hidden experiment substrate,
    mirror the artifact into that manifest as optional bookkeeping
- Typical commands:
  - `perf stat -x, --no-big-num -- <workload>`
  - `perf stat -x, --no-big-num -p <pid> --timeout 10000`
  - `perf stat -x, --no-big-num -a -G <cgroup> --timeout 10000`
- Record:
  - collector command
  - target selector
  - direct metrics such as `ipc`, `cpi`, `cycles`, `instructions`
  - whether the metric basis is direct or proxy-only

## `schedstat`
- Questions:
  - Is run queue time growing?
  - Are tasks waiting longer before running?
- Typical commands:
  - `cat /proc/schedstat`
  - `cat /proc/<pid>/schedstat`
- Record:
  - relevant CPUs or tasks
  - capture phase
  - parsed queue or wait counters when available

## `psi`
- Questions:
  - Is the bottleneck scheduler queueing or broader system pressure?
  - Is CPU pressure persistent or bursty?
- Typical commands:
  - `cat /proc/pressure/cpu`
  - `cat /proc/pressure/io`
  - `cat /proc/pressure/memory`
- Record:
  - which pressure dimension matters
  - whether the signal weakens a scheduler-only hypothesis

## `perf_sched`
- Questions:
  - Are wakeups landing on the wrong CPUs?
  - Is migration churn excessive?
  - Is dispatch ordering itself the issue?
- Typical commands:
  - `perf sched record -- <workload>`
  - `perf sched timehist`
  - `perf sched latency`
- Record:
  - short trace windows only
  - artifact paths for raw record plus derived summary

## `perf_record`
- Questions:
  - Do we need sampled hotspots to explain a scheduler symptom?
  - Is the issue actually outside the scheduler fast path?
- Typical commands:
  - `perf record -g -- <workload>`
  - `perf report --stdio`
- Record:
  - capture scope
  - top DSOs or functions
  - why sampling was necessary

## `bpf_trace`
- Questions:
  - Do we need focused kernel event correlation that `perf sched` cannot provide?
- Typical examples:
  - short-lived tracepoints around wakeup, enqueue, dispatch, or cgroup transitions
- Record:
  - exact script path or command
  - kernel version assumptions
  - why a focused BPF view was needed

## `custom`
- Use when the command family does not fit the built-in kinds.
- Keep the record explicit anyway:
  - collector
  - artifacts
  - summary

## Research anchors
- `perf-stat(1)` documents the machine-readable `-x` CSV-style output plus
  `-p`, `-G`, and `--timeout`, which makes `perf stat` the right low-overhead
  first-pass collector for `sched-claw`.
- The kernel `sched_ext` docs justify starting with low-overhead evidence first:
  scheduler switching is dynamic and safe fallback exists, so short controlled
  evidence windows are better than always-on tracing.
