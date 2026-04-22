---
name: "llvm-clang-build-tuning"
description: "Use when demonstrating or tuning a compile-heavy LLVM/clang build workload with sched-claw. Covers the demo launcher, direct build metrics, and the fallback path to IPC/CPI proxies."
aliases:
  - "llvm-build-demo"
  - "clang-build-tuning"
tags:
  - "sched-claw"
  - "llvm"
  - "clang"
  - "build"
  - "scheduler"
---

# LLVM/Clang Build Tuning

## When to use
- Demonstrate sched-claw on a compile-heavy LLVM/clang workload.
- Tune scheduler behavior for a large parallel build where overall wall-clock build time is the main objective.
- Need a workload launcher that writes direct metrics and can be replayed by the agent.

## Direct metrics first
- Primary direct metric: `build_seconds:minimize`
- Secondary direct metric: `configure_seconds:minimize`
- Preferred proxy metrics when direct timing is untrustworthy: `ipc:maximize`, `cpi:minimize`

## Demo entrypoints
- Demo wrapper:
  - `apps/sched-claw/scripts/demos/llvm-clang-autotune.sh`
- Workload launcher:
  - `apps/sched-claw/scripts/workloads/run-llvm-clang-build.sh`

## Workflow
1. Confirm the LLVM source tree path.
   - The launcher accepts either the monorepo root or the `llvm/` source root.
2. Bootstrap the demo experiment.
   - The demo wrapper already calls `sched-claw experiment init ...` with `build_seconds` as the direct primary metric.
3. Use direct build timing unless you have a concrete reason not to.
   - Read `metrics.env` from the workload artifact directory.
   - Only switch to `proxy_estimate` when wall-clock timing is contaminated by unrelated host noise or the run is incomplete.
4. Favor conservative scheduler candidates.
   - For compile-heavy workloads, `balanced_queue` and `dsq_locality` are the normal starting points.
   - Treat locality, migration churn, and queue balance as the main policy levers.
5. Keep rollout criteria explicit.
   - This demo may build for a long time. State the maximum activation window and the rollback trigger before using the daemon.

## Rules
- Do not pretend IPC/CPI are direct business metrics. They are fallback proxies here.
- Keep the workload replayable through the launcher script instead of ad hoc shell commands.
- Preserve the artifact path used by the launcher so the agent can read `build.log`, `configure.log`, and `metrics.env`.

## Reference Material
- `references/demo-contract.md`
