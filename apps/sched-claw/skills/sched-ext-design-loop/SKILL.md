---
name: "sched-ext-design-loop"
description: "Use when the task requires turning workload evidence into a new sched-ext scheduler, validating it against CFS, and rolling it out through the privileged daemon with explicit rollback criteria."
aliases:
  - "sched-ext-loop"
tags:
  - "sched-ext"
  - "ebpf"
  - "scheduler"
---

# sched-ext Design Loop

## When to use
- Design or modify a `sched-ext` scheduler from performance evidence.
- Decide what policy or queueing changes should replace the current CFS behavior for a target workload.
- Activate or stop the scheduler through the daemon after code changes are built.

## Workflow
1. Start from the workload contract.
   - Name the optimization target: latency, fairness, locality, throughput, tail behavior, or isolation.
   - Record what CFS is currently doing wrong.
2. Translate evidence into policy knobs.
   - Wakeup placement and CPU selection
   - Dispatch queue topology
   - Time-slice budgeting or starvation protection
   - Migration control and cache locality
   - cgroup or workload-class separation
3. Keep the implementation loop explicit.
   - Edit code with normal file tools.
   - Build with normal shell tools.
   - Use `sched_ext_daemon` only for `status`, `activate`, `stop`, and `logs`.
4. Compare against the CFS baseline with the same workload and evidence set.
   - If the new scheduler regresses the primary goal or introduces a new bottleneck, stop it and return to the last stable point.
5. Always define rollback criteria before activation.
   - Latency threshold, throughput drop, instability, watchdog fallback, or log evidence that invalidates the rollout.

## Rules
- Do not use the daemon as a generic privileged shell.
- Keep generated scheduler code and rollout commands reproducible from the workspace.
- State whether each conclusion is a fact, inference, or hypothesis.
- Prefer a single active scheduler experiment at a time so comparisons stay attributable.
