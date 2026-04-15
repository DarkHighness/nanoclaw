---
name: "linux-performance-analysis"
description: "Use when a task requires analyzing Linux performance evidence collected with perf, sysstat, or eBPF. Apply workload characterization, the USE method, top-down microarchitecture analysis, CPI or IPC interpretation, off-CPU analysis, and bottleneck ranking. Do not trigger for raw collection-only tasks without interpretation."
---

# Linux Performance Analysis

## When to use
- Analyze Linux performance evidence after counters, traces, or profiles were collected.
- Determine whether a workload is CPU-bound, memory-bound, I/O-bound, scheduler-bound, lock-bound, or network-limited.
- Turn raw `perf`, `sysstat`, or eBPF artifacts into ranked bottlenecks and next experiments.

## Read before analyzing
- `references/analysis-methods.md` for the investigation order and bottleneck ranking method.
- `references/topdown-and-cpi.md` for IPC/CPI, top-down, and counter interpretation constraints.

## Workflow
1. Start from the workload and symptom, not the tool.
   - State the bad outcome: latency, throughput collapse, CPU burn, queue growth, or jitter.
   - State the workload phase and whether the evidence came from steady-state or a transition.
2. Apply USE across every relevant resource.
   - CPU, memory, block I/O, network, scheduler, and locks are all candidates.
   - Distinguish utilization from saturation. High utilization alone is not proof of the limiter.
3. Decide whether the workload is on-CPU or waiting.
   - If the CPU is busy and the process spends most time on-CPU, move to `perf stat`, flame graphs, and top-down.
   - If latency is high without corresponding CPU saturation, look for off-CPU time, queueing, blocking, reclaim, lock waits, or I/O latency.
4. For CPU-bound paths, move from simple to specific.
   - Start with IPC/CPI, branch miss, and cache miss signals.
   - Then use top-down categories to decide whether the loss is frontend, backend, speculation, or non-retiring work.
   - Only after that descend into flame graphs, hot functions, or source lines.
5. Build a bottleneck argument.
   - Facts: directly observed metrics and stack evidence.
   - Inferences: what the metrics imply about the limiter.
   - Unknowns: what remains unproven and what extra evidence would settle it.
6. Rank bottlenecks by impact and confidence.
   - Name the primary limiter, secondary contributors, and discarded hypotheses.
   - End with the next confirming experiment.

## Analysis rules
- Do not interpret CPI in isolation across different workloads or microarchitectures.
- Do not treat top-down percentages as self-sufficient root cause; they narrow the search space.
- Use at least two evidence paths for a strong conclusion when possible, such as counters plus stacks, or device metrics plus latency histograms.
- If the workload changed during capture, say so and reduce confidence.

## Output expectations
- Produce a ranked bottleneck list.
- Separate fact, inference, and unknown explicitly.
- State confidence and the limits of the evidence.
