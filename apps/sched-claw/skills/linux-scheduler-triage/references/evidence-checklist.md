# Scheduler Evidence Checklist

## Minimum evidence set
- kernel version and CPU topology
- workload command line or launcher
- bad metric definition
- one clean CFS baseline run
- per-CPU activity summary
- queueing or runnable-delay evidence

## When to stop and say "not scheduler-dominant"
- PSI shows memory or IO pressure dominating the bad phase
- CPU utilization is high but run queue, wakeup, and migration signals stay normal
- the workload regresses before becoming runnable
- the issue disappears when the workload input changes but the scheduler evidence does not

## Questions that should be answered before policy design
- Is the dominant issue wakeup latency, unfairness, migration churn, or class interference?
- Is the issue global or restricted to a CPU subset, NUMA node, or cgroup?
- Which evidence item would falsify the current leading hypothesis?
