---
name: "sched-workload-contract"
description: "Use when a tuning request needs to be turned into a durable workload contract before any sched-ext code is changed. Covers selectors, direct metrics, proxy metrics, guardrails, and saved contract hygiene."
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
- The agent is about to create or update a saved workload contract.
- A tuning request mixes direct metrics with proxy metrics unclearly.

## Read before acting
- `references/selector-and-metric-policy.md`

## Workflow
1. Pin the target selector first.
   - Use exactly one primary selector: script, pid, uid, gid, or cgroup.
   - If a script launcher exists, keep the exact argv and env durable in a note, JSON, or artifact manifest.
2. Separate direct metrics from proxy metrics.
   - Prefer throughput and latency when the workload exposes them.
   - Use IPC/CPI only when direct metrics do not exist or are known to be untrustworthy.
3. Make the policy explicit in a saved contract artifact.
   - record the primary metric, goal, optional guardrails, and whether the basis is direct or proxy
   - use `proxy_estimate` wording only when proxy metrics are actually needed
   - `scripts/scaffold_workload_contract.sh` is the default helper when you want a durable TOML contract instead of ad-hoc prose
   - `scripts/validate_workload_contract.py` is the default helper when the contract exists but you need to prove the selector, metric basis, and proxy/direct split are still coherent
4. Record the evaluation scope.
   - steady state vs startup
   - single-tenant vs noisy host
   - what regression is unacceptable
5. End with a typed contract, not prose only.

## Rules
- Do not hide selector choice in the transcript.
- Do not present IPC/CPI as direct business metrics.
- Do not begin candidate generation until the workload contract is durable.

## Output checklist
- explicit selector
- explicit primary metric and goal
- guardrails
- whether the basis is direct or proxy
- contract artifact path

## Reference Material
- `references/selector-and-metric-policy.md`

## Optional Helper Scripts
- `scripts/scaffold_workload_contract.sh`
  - writes a durable TOML workload contract scaffold
  - keeps selector, direct metric policy, proxy metrics, and guardrails explicit
  - preferred when a tuning request is about to move from discussion into durable artifacts
- `scripts/validate_workload_contract.py`
  - validates a contract TOML file and reports selector, goal, basis, and proxy/direct mismatches
  - can emit JSON or Markdown if the next step needs a durable validation note instead of ad-hoc transcript text
