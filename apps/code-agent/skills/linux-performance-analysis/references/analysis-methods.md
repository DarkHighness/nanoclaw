# Linux performance analysis methods

This reference synthesizes:
- Brendan Gregg, "Performance Analysis Methodology", https://www.brendangregg.com/methodology.html
- Brendan Gregg, "The USE Method: Linux", https://www.brendangregg.com/USEmethod/use-linux.html
- Brendan Gregg, "Active Benchmarking", https://www.brendangregg.com/activebenchmarking.html
- Brendan Gregg, "Off-CPU Flame Graphs", https://www.brendangregg.com/FlameGraphs/offcpuflamegraphs.html

## Recommended analysis order

1. Problem statement
   - What degraded?
   - When?
   - Under what workload?
2. Workload characterization
   - Throughput, request mix, concurrency, data size, burst shape, pinned CPUs, container limits.
3. USE pass
   - Apply Utilization, Saturation, Errors to CPU, memory, storage, network, scheduler, and locks.
4. Decide on-CPU versus waiting
   - High CPU alone is not enough.
   - Look for queue growth, blocking, and device or scheduler latency.
5. Drill down
   - CPU path: counters, top-down, flame graphs, hot code.
   - Wait path: off-CPU stacks, run queue latency, lock profiles, I/O histograms, retransmits, reclaim activity.
6. Rank limiters and state unknowns

## USE method reminders
- Utilization asks "how busy is the resource?"
- Saturation asks "how much work is waiting?"
- Errors asks "is the resource failing or retrying?"

USE is a screening method. It is meant to prevent premature fixation on CPU hotspots when the workload is actually blocked on something else.

## Active benchmarking

When analyzing benchmark runs:
- Capture system evidence while the benchmark is running.
- Do not report only the benchmark score.
- State the limiting factor and the evidence supporting it.
- Note benchmark configuration, input size, pinning, and thermal or frequency behavior if relevant.

## Off-CPU analysis

Use off-CPU analysis when:
- Tail latency is high but CPU utilization is not fully saturated.
- Threads spend time sleeping, blocking on locks, or waiting on storage/network.
- Scheduler delay or wakeup latency is suspected.

Off-CPU flame graphs and eBPF latency histograms are especially useful when averages hide bimodal behavior or long tails.

## Bottleneck ranking template

For each candidate limiter:
- Facts: exact metrics, stacks, histograms, or error counts.
- Inference: what these facts imply.
- Confidence: high, medium, or low.
- Disproof path: what evidence would falsify the current interpretation.
