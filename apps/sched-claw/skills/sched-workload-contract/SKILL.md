---
name: "sched-workload-contract"
description: "Use when a tuning request needs to be turned into a durable workload contract before any sched-ext code is changed. Covers selectors, direct metrics, proxy metrics, guardrails, and experiment manifest hygiene."
aliases:
  - "workload-contract"
tags:
  - "sched-claw"
  - "workload"
  - "metrics"
---

# sched Workload Contract

## When to use
- A workload has been named, but the target selector or success metric is still vague.
- The agent is about to create or update an experiment manifest.
- A tuning request mixes direct metrics with proxy metrics unclearly.

## Read before acting
- `references/selector-and-metric-policy.md`

## Workflow
1. Pin the target selector first.
   - Use exactly one primary selector: script, pid, uid, gid, or cgroup.
   - If a script launcher exists, keep the exact argv and env durable in the experiment manifest.
2. Separate direct metrics from proxy metrics.
   - Prefer throughput and latency when the workload exposes them.
   - Use IPC/CPI only when direct metrics do not exist or are known to be untrustworthy.
3. Make the policy explicit in the experiment manifest.
   - `sched-claw experiment init ...`
   - `--primary-metric`, `--primary-goal`, optional `--guardrail`
   - `--performance-basis proxy_estimate` only when proxy metrics are actually needed
4. Record the evaluation scope.
   - steady state vs startup
   - single-tenant vs noisy host
   - what regression is unacceptable
5. End with a typed contract, not prose only.

## Rules
- Do not hide selector choice in the transcript.
- Do not present IPC/CPI as direct business metrics.
- Do not begin candidate generation until the experiment contract is durable.

## Output checklist
- explicit selector
- explicit primary metric and goal
- guardrails
- whether the basis is direct or proxy
- experiment id and manifest path

## Reference Material
- `references/selector-and-metric-policy.md`
