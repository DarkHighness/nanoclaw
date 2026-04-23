---
name: "mysql-sysbench-tuning"
description: "Use when demonstrating or tuning a sysbench + MySQL workload with sched-claw. Covers the demo launcher, direct throughput and latency metrics, and workload-specific scheduler implications."
aliases:
  - "mysql-demo"
  - "sysbench-mysql-tuning"
tags:
  - "sched-claw"
  - "mysql"
  - "sysbench"
  - "database"
  - "scheduler"
---

# MySQL Sysbench Tuning

## When to use
- Demonstrate sched-claw on a MySQL OLTP workload driven by sysbench.
- Tune scheduler behavior for throughput-sensitive database pressure while keeping latency regressions visible.

## Role
This is a workload profile, not a foundational workflow skill. It should supply
workload-specific metrics, likely policy levers, and launcher details, then
compose the generic contract, collection, analysis, policy, codegen, and
rollout skills.

## Compose with
- `sched-workload-contract`
- `sched-perf-collection`
- `sched-perf-analysis`
- `sched-policy-mapping`
- `sched-ext-codegen`
- `sched-ext-run-evaluation`
- `sched-ext-rollout-safety`

## Workload-specific facts
- Primary direct metric: `transactions_per_sec:maximize`
- Secondary direct metric: `queries_per_sec:maximize`
- Latency guardrail: `p95_latency_ms:minimize`
- Optional auxiliary metric: `avg_latency_ms:minimize`
- Common policy starting points:
  - tail-latency protection
  - queue balance
  - workload isolation

## Entrypoints
- Demo wrapper:
  - `apps/sched-claw/scripts/demos/mysql-sysbench-autotune.sh`
- Workload launcher:
  - `apps/sched-claw/scripts/workloads/run-mysql-sysbench.sh`
- Dockerized sysbench runner recipe:
  - `apps/sched-claw/scripts/docker/sysbench-runner.Dockerfile`

## Guardrails
- Prefer direct sysbench metrics over proxy counters whenever they are available.
- Use short activation windows first and stop immediately on severe latency spikes, stalls, or startup failures.

## Rules
- Treat sysbench throughput and latency as direct metrics, not proxies.
- Keep container lifecycle and benchmark lifecycle reproducible through the launcher script.
- Preserve the launcher artifact path so the agent can read `run.log`, `prepare.log`, and `metrics.env`.

## Reference Material
- `references/demo-contract.md`
