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
- Need a launcher that can start an ephemeral docker MySQL instance or target an existing host MySQL.

## Direct metrics first
- Primary direct metric: `transactions_per_sec:maximize`
- Secondary direct metric: `queries_per_sec:maximize`
- Latency guardrail: `p95_latency_ms:minimize`
- Optional auxiliary metric: `avg_latency_ms:minimize`

## Demo entrypoints
- Demo wrapper:
  - `apps/sched-claw/scripts/demos/mysql-sysbench-autotune.sh`
- Workload launcher:
  - `apps/sched-claw/scripts/workloads/run-mysql-sysbench.sh`
- Dockerized sysbench runner recipe:
  - `apps/sched-claw/scripts/docker/sysbench-runner.Dockerfile`

## Workflow
1. Pick the MySQL source.
   - `--mode docker` is the self-contained demo path.
   - `--mode host` is for an already-running MySQL instance.
   - The default launcher path also builds a local sysbench runner image, so the demo does not require a host-installed `sysbench` binary.
2. Read the demo workload context.
   - The demo wrapper writes a workload-context note next to its artifacts with the launcher path, direct metrics, guardrails, and rollback notes.
   - Do not collapse throughput and latency into a single vague “performance” label.
3. Prefer direct sysbench metrics.
   - Read `metrics.env` from the workload artifact directory.
   - Only fall back to proxy counters if sysbench output is unavailable or invalid.
4. Match candidate choice to the observed bottleneck.
   - `latency_guard` when p95 wakeup or tail latency is the main problem.
   - `balanced_queue` when throughput collapses under queue imbalance.
   - `cgroup_lane` when the workload must be isolated from background neighbors.
5. Keep database rollout conservative.
   - Use short activation windows first.
   - Stop immediately on severe latency spikes, stalls, or startup failures.

## Rules
- Treat sysbench throughput and latency as direct metrics, not proxies.
- Keep container lifecycle and benchmark lifecycle reproducible through the launcher script.
- Preserve the launcher artifact path so the agent can read `run.log`, `prepare.log`, and `metrics.env`.

## Reference Material
- `references/demo-contract.md`
