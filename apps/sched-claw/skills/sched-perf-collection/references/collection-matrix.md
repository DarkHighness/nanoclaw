# Collection Matrix

Use the lightest collector that can falsify the current hypothesis.

## `perf_stat`
- Questions:
  - Is retiring efficiency low?
  - Are cycles or stalled frontend/backend dominating?
  - Did IPC or CPI move in the expected direction?
- Typical commands:
  - `perf stat -d -d -- <workload>`
  - `perf stat -p <pid> -d -d sleep 10`
- Record:
  - collector command
  - target selector
  - direct metrics such as `ipc`, `cpi`, `cycles`, `instructions`

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
