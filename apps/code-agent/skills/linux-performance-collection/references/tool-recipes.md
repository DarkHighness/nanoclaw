# Tool recipes for Linux performance collection

## perf counters

Use counters first:

```bash
perf stat -d -d -d -- <command>
perf stat -d -d -d -p <pid> sleep 30
perf stat -e cycles,instructions,branches,branch-misses,cache-references,cache-misses -- <command>
```

Use this to gather IPC, CPI, cache miss ratios, and branch miss ratios before sampling.

## perf profiling

Use sampling only when attribution is required:

```bash
perf record -F 99 -g -- <command>
perf record -F 99 -g -p <pid> -- sleep 30
perf report
```

Keep frequency conservative first. Narrow to a PID or pinned CPU if the host is noisy.

## Scheduler and lock views

```bash
perf sched record -- <command>
perf sched latency
perf lock record -- <command>
perf lock report
```

Use these when run queue growth, tail latency, or lock contention is suspected.

## sysstat views

```bash
sar -u 1
sar -r 1
sar -b 1
sar -n DEV 1
sar -n TCP,ETCP 1
iostat -xz 1
mpstat -P ALL 1
pidstat -urdw 1
```

These are low-overhead and suitable for repeated snapshots or longer benchmark windows.

## eBPF entry points

Prefer existing tools for common questions:
- `bpftrace` one-liners for fast histograms or per-event counts.
- BCC tools such as `runqlat`, `biolatency`, `offcputime`, and `tcpretrans` when installed.

Use eBPF when averages hide the real issue. Histograms and per-stack aggregation are usually more informative than a single mean value.
