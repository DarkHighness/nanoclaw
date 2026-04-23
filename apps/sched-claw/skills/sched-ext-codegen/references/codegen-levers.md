# Codegen Levers

Map evidence to the smallest plausible scheduler change.

## Locality pressure
- Signals:
  - migration churn
  - low IPC or higher CPI
  - wakeups frequently land off the warm CPU
- Levers:
  - stronger same-cpu wakeup bias
  - stricter migration thresholds
  - per-CPU, per-LLC, or topology-aware DSQ structure

## Tail-latency sensitivity
- Signals:
  - p95 or p99 grows while average throughput is acceptable
  - latency-sensitive tasks lose to bulk work
- Levers:
  - shorter slices for latency class
  - priority-aware dispatch queue separation
  - faster preemption or wakeup preference for critical tasks

## Throughput collapse with okay latency
- Signals:
  - throughput falls first
  - CPUs idle while work remains elsewhere
- Levers:
  - less sticky affinity
  - more aggressive idle pull
  - wider sharing across DSQs

## Isolation / cgroup interference
- Signals:
  - noisy neighbors damage the target workload
  - cgroup or class boundaries matter more than global fairness
- Levers:
  - class-aware DSQs
  - cgroup-tagged dispatch
  - quota or starvation protection per workload class

## Industrial anchors
- `sched_ext` exposes DSQ topology, wakeup placement, enqueue, and dispatch as
  first-class hooks, so those are the preferred mutation surfaces.
- `scx_lavd` shows an industrial-quality example of mapping latency-criticality
  into both deadline urgency and slice sizing instead of a single score.
- `scx_loader` shows that operator intent is often expressed as a small set of
  runtime modes, not arbitrary root operations. Candidate code should be
  narrow enough to fit controlled rollout and rollback surfaces.

## Durable design record
Before editing code, keep these fields explicit:
- evidence ids
- analysis ids
- candidate id
- policy levers
- invariants
- code targets
- risks
- fallback criteria
