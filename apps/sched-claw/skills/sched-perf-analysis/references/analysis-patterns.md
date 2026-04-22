# Analysis Patterns

These are heuristics, not automatic truths. Treat them as candidate interpretations.

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
