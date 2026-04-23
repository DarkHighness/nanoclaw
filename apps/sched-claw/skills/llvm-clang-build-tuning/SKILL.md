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

## Role
This is a workload profile, not a foundational workflow skill. It should supply
workload-specific selectors, direct metrics, and likely scheduler levers, then
compose the generic skills that do the actual contract, collection, analysis,
policy mapping, codegen, and rollout work.

## Compose with
- `sched-workload-contract`
- `sched-perf-collection`
- `sched-perf-analysis`
- `sched-policy-mapping`
- `sched-ext-codegen`
- `sched-ext-run-evaluation`
- `sched-ext-rollout-safety`

## Workload-specific facts
- Primary direct metric: `build_seconds:minimize`
- Secondary direct metric: `configure_seconds:minimize`
- Preferred fallback proxies: `ipc:maximize`, `cpi:minimize`
- Common policy starting points: locality, migration control, and queue balance

## Entrypoints
- Demo wrapper:
  - `apps/sched-claw/scripts/demos/llvm-clang-autotune.sh`
- Workload launcher:
  - `apps/sched-claw/scripts/workloads/run-llvm-clang-build.sh`

## Guardrails
- Keep direct build timing primary unless the workload context explicitly says it is contaminated.
- Keep rollout windows bounded because large LLVM builds can run for a long time.

## Rules
- Do not pretend IPC/CPI are direct business metrics. They are fallback proxies here.
- Keep the workload replayable through the launcher script instead of ad hoc shell commands.
- Preserve the artifact path used by the launcher so the agent can read `build.log`, `configure.log`, and `metrics.env`.

## Reference Material
- `references/demo-contract.md`
