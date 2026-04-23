# Analysis Patterns

These are heuristics, not automatic truths. Treat them as candidate interpretations.

## Metric policy
- Direct latency or throughput metrics outrank proxy CPU metrics.
- IPC, CPI, and top-down style counters are proxy signals. Use them when the
  workload has no trustworthy direct service metric, or when they explain why a
  direct metric moved.
- If direct metrics and proxy metrics disagree, do not silently override the
  workload outcome with PMU counters.
- When a capture already includes `instructions`, `cycles`, `branches`,
  `branch-misses`, `cache-references`, or `cache-misses`, derive IPC, CPI, and
  miss-rate helpers directly instead of retyping those formulas in each turn.

## Low IPC / high CPI with heavy migration churn
- Plausible meaning:
  - locality is weak
  - tasks are paying cache refill cost after migration
- Disconfirmers:
  - hotspots show pure memory-bandwidth saturation
  - the issue persists even when migrations stay low

## High wakeup latency with modest CPU utilization
- Plausible meaning:
  - wakeup placement or dispatch ordering is suboptimal
  - work is available but not being run promptly
- Disconfirmers:
  - run queue is actually empty during the bad phase
  - the delay comes from locks or IO completion, not scheduling

## Tail latency regresses while throughput is flat
- Plausible meaning:
  - fairness or starvation control is weak
  - background work interferes with latency-sensitive tasks
- Disconfirmers:
  - tail regressions align with external IO pressure or reclaim spikes

## Throughput regresses after stronger locality bias
- Plausible meaning:
  - affinity is too sticky
  - cross-CPU pull is now too conservative
- Disconfirmers:
  - the regression is caused by a verifier-safe but functionally broken scheduler change

## PSI says CPU pressure is low, but latency is still bad
- Plausible meaning:
  - pure CPU saturation is not the driver
  - investigate wakeup ordering, locking, or non-scheduler causes
- Disconfirmers:
  - the PSI window missed the bad phase

## Useful durable record shape
- facts:
  - direct counters and trace summaries
- inferences:
  - what those facts imply about policy levers
- unknowns:
  - what still requires collection
- recommendations:
  - next capture, code change, or rollback gate

## Research anchors
- The Top-Down analysis paper motivates using structured PMU hierarchies to
  separate frontend, backend, bad speculation, and retiring bottlenecks instead
  of reading raw counters in isolation.
- `perf-stat(1)` documents `--topdown` and the caveats around root rights,
  interval mode, and inconsistent bottlenecks on changing workloads.
