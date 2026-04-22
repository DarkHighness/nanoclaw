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
   - Record what CFS is currently doing wrong and what evidence supports that claim.
2. Convert evidence into policy levers.
   - Wakeup placement and CPU selection
   - Dispatch queue topology
   - Time-slice budgeting or starvation protection
   - Migration control and cache locality
   - cgroup or workload-class separation
   - fallback or partial-switch strategy when full replacement is too risky
3. Make the implementation plan explicit before editing code.
   - Which source files or scheduler examples you are borrowing from
   - What state lives in BPF maps, DSQs, or per-task/per-cpu storage
   - What invariant should hold after each scheduling decision
4. Keep the implementation loop explicit.
   - Edit code with normal file tools.
   - Build with normal shell tools.
   - Capture build commands and compiler output in the artifact directory.
   - Use `sched_ext_daemon` only for `status`, `activate`, `stop`, and `logs`.
5. Define rollout and rollback criteria before activation.
   - Latency threshold, throughput drop, instability, watchdog fallback, log evidence, or CPU stall conditions that invalidate the rollout.
   - State the maximum time you are willing to leave the experimental scheduler active.
6. Run the controlled activation loop.
   - Confirm daemon `status`.
   - `activate` the scheduler with a human-readable label.
   - Inspect `logs` immediately for startup failures or debug dumps.
   - Run the target workload with the exact same command set used for the CFS baseline.
   - `stop` the scheduler as soon as the verification window ends or a rollback trigger fires.
7. Compare against CFS using the same evidence set.
   - If the new scheduler regresses the primary goal or introduces a new bottleneck, stop it and return to the last stable point.
   - Separate "policy failed" from "measurement insufficient" so the next iteration is scoped correctly.

## Output Checklist
- policy summary with explicit mapping from evidence to scheduler behavior
- changed source files and build command
- activation label and daemon command arguments
- startup log excerpt and stop status
- before/after comparison against the CFS baseline
- next action: keep, revise, or rollback

## Rules
- Do not use the daemon as a generic privileged shell.
- Keep generated scheduler code and rollout commands reproducible from the workspace.
- State whether each conclusion is a fact, inference, or hypothesis.
- Prefer a single active scheduler experiment at a time so comparisons stay attributable.
- If the sched-ext ABI or helper surface looks version-sensitive, say so and tie the conclusion to the tested kernel version.

## Reference Material
- `references/official-docs.md`
- `references/rollout-checklist.md`
