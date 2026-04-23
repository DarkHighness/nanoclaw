# Lever Mapping

Turn evidence into the narrowest scheduler policy statement that can still be
tested.

## Objective before lever
- Name the objective first.
- Keep guardrails explicit.
- If the objective is still fuzzy, go back to the workload contract.

## Typical mappings

### Locality loss
- Signals:
  - heavy migration churn
  - lower IPC or higher CPI after wakeups
  - wakeups regularly land on cold CPUs
- Levers:
  - stronger same-cpu wakeup bias
  - stricter migration thresholds
  - topology-aware DSQ separation

### Tail latency growth
- Signals:
  - p95 or p99 grows while average throughput stays acceptable
  - latency-sensitive tasks wait behind bulk work
- Levers:
  - shorter slices for the latency class
  - faster wakeup preference
  - dedicated DSQ or dispatch lane for the critical class

### Throughput collapse
- Signals:
  - throughput drops first
  - CPUs go idle while runnable work exists elsewhere
- Levers:
  - weaker affinity
  - stronger idle pull
  - broader DSQ sharing

### Isolation failure
- Signals:
  - noisy neighbors interfere with the target workload
  - cgroup or workload-class boundaries dominate fairness needs
- Levers:
  - class-aware DSQs
  - cgroup-tagged dispatch
  - starvation or quota protection per class

## Keep these fields durable
- evidence refs
- objective
- candidate levers
- invariants
- open questions
- invalidation conditions
