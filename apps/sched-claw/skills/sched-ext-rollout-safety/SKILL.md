---
name: "sched-ext-rollout-safety"
description: "Use when a sched-ext candidate is about to be activated through the privileged daemon. Covers rollout gates, daemon usage limits, rollback triggers, and how to keep deployment evidence attributable."
aliases:
  - "rollout-safety"
tags:
  - "sched-ext"
  - "daemon"
  - "rollout"
---

# sched-ext Rollout Safety

## When to use
- A candidate build is ready and activation is the next step.
- A daemon-driven run or deployment needs explicit rollback criteria.
- A candidate looked good in scoring, but production-like rollout risk still needs to be controlled.

## Read before acting
- `references/activation-checklist.md`

## Workflow
1. Check the latest build and verifier result.
   - do not proceed if the latest record failed unless the operator explicitly overrides the gate
2. Make the rollout window explicit.
   - what workload phase is being observed
   - what metric or log signal forces rollback
   - how long the candidate may stay active
   - `scripts/scaffold_rollout_plan.sh` is the default helper when you want those gates durable before activation
3. Use the daemon narrowly.
   - `status`
   - `activate`
   - `logs`
   - `stop`
4. Capture the startup evidence immediately.
   - read daemon logs after activation
   - keep the activation label durable in the experiment or artifact trail
5. Stop early when evidence is bad.
   - guardrail breach
   - startup failure
   - instability
   - measurement corruption

## Rules
- Do not use the daemon as a generic privileged shell.
- Do not leave multiple candidate rollouts running concurrently if attribution matters.
- A good score without a controlled rollout window is still incomplete operational evidence.

## Output checklist
- activation label
- latest build or verifier gate result
- rollback triggers
- daemon log status
- final keep or rollback decision

## Reference Material
- `references/activation-checklist.md`

## Optional Helper Scripts
- `scripts/scaffold_rollout_plan.sh`
  - writes a durable Markdown rollout plan with lease, guardrails, rollback triggers, and daemon command placeholders
  - preferred when a candidate is close to activation and the missing piece is operational clarity rather than more host-side orchestration
