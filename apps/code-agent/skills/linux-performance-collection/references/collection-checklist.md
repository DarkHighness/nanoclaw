# Linux performance collection checklist

This checklist is grounded in:
- Brendan Gregg, "The USE Method: Linux", https://www.brendangregg.com/USEmethod/use-linux.html
- Brendan Gregg, "Linux Performance Analysis in 60,000 Milliseconds", https://www.brendangregg.com/Articles/Netflix_Linux_Perf_Analysis_60s.pdf
- sysstat official repository, https://github.com/sysstat/sysstat

## Before the capture
- Record hostname, kernel, distro, CPU model, NUMA layout, and container/cgroup scope.
- Record the workload trigger, exact command, input size, and expected bad behavior.
- Check whether `perf` access is restricted (`perf_event_paranoid`) and whether `/sys/kernel/btf/vmlinux` exists for eBPF CO-RE use.

## First-pass triage set

Prefer this order before deeper tracing:

```bash
uptime
dmesg | tail
vmstat 1
mpstat -P ALL 1
pidstat 1
iostat -xz 1
free -m
sar -n DEV 1
sar -n TCP,ETCP 1
top -H
```

Interpretation intent:
- `vmstat 1`: run queue, paging, system time, blocked tasks.
- `mpstat -P ALL 1`: per-CPU imbalance, steal time, interrupt load.
- `pidstat 1`: per-process CPU, faults, context switches, I/O.
- `iostat -xz 1`: storage utilization, queue depth, await, service time.
- `sar -n DEV 1`: network throughput and interface utilization.
- `sar -n TCP,ETCP 1`: retransmits, resets, listen overflow symptoms.

## USE-oriented mapping

For each resource, ask for Utilization, Saturation, and Errors:

- CPU
  - Utilization: `mpstat`, `pidstat`, `top`
  - Saturation: `vmstat` run queue, scheduler latency, involuntary context switching
  - Errors: machine checks or kernel warnings in `dmesg`
- Memory
  - Utilization: `free -m`, `vmstat`
  - Saturation: reclaim, swap activity, major faults, OOM pressure
  - Errors: allocation failures, OOM kills
- Block I/O
  - Utilization: `%util`, device busy time in `iostat`
  - Saturation: queue depth, `await`, merges, latency histograms
  - Errors: I/O error lines in `dmesg`
- Network
  - Utilization: interface throughput from `sar -n DEV`
  - Saturation: drops, backlog, retransmits, socket queue pressure
  - Errors: `sar -n EDEV`, driver/kernel error counters

## When to add perf
- Need CPI/IPC, branch/cache signals, or on-CPU stacks.
- Need to prove whether the workload is CPU-bound or waiting elsewhere.

## When to add eBPF
- Need latency distributions instead of averages.
- Need off-CPU time, queueing attribution, or per-event causality.
- Need stable low-overhead tracing for a narrow hypothesis.

## Artifact conventions
- Save commands and outputs together.
- Include timestamps in filenames or adjacent metadata.
- Keep benchmark inputs, CPU pinning, and environment flags next to the capture.
