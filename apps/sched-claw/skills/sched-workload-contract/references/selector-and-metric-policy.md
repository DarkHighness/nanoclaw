# Selector and Metric Policy

## Selector hierarchy
- script: preferred when a reproducible launcher exists
- pid: useful for already-running single targets
- uid/gid: useful for multi-process workload classes
- cgroup: preferred for service slices and resource-isolated workloads

## Metric policy
- direct first: throughput, latency, tail latency, completion time
- proxy second: IPC, CPI, cache-miss rates

## Guardrail policy
- A candidate is not "better" if the primary metric improves while a declared guardrail is materially violated.
