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

## Role
This is a composition skill. It should decide which implementation, verification,
evaluation, and rollout primitives to compose next instead of restating the full
workflow each time.

## Compose with
- `sched-workload-contract`
  - when objective, selector, or direct metric basis is still unclear
- `sched-policy-mapping`
  - when evidence exists but levers or invariants are not explicit yet
- `sched-ext-codegen`
  - when a candidate path or edit surface must be created
- `sched-ext-build-verify`
  - when source exists and build or verifier results are the next gate
- `sched-ext-run-evaluation`
  - when baseline vs candidate evidence must be compared
- `sched-ext-rollout-safety`
  - when daemon activation and rollback windows need operational control

## Responsibilities
1. Keep the target objective explicit.
2. Choose the next primitive skill, not one giant workflow.
3. Keep artifact links explicit between policy mapping, code, build output, run output, and rollout logs.
4. Keep privileged daemon use narrow.

## Default helper choices
- `../sched-ext-codegen/scripts/scaffold_design_brief.sh`
  - when evidence must become a durable code-facing brief
- `../sched-ext-codegen/scripts/scaffold_edit_checklist.sh`
  - when a narrow code edit needs explicit hook, DSQ, map, and rollout coverage
- `../sched-ext-codegen/scripts/scaffold_sched_ext_candidate.sh`
  - when a fresh candidate directory is required

## Output Checklist
- policy summary with explicit mapping from evidence to scheduler behavior
- if available, the design note path and the evidence or analysis artifacts it cites
- changed source files and build command
- activation label and daemon command arguments
- startup log excerpt and stop status
- before/after comparison against the CFS baseline
- next action: keep, revise, or rollback
- if available, the current comparison output and whether the candidate should be kept, revised, or rolled back

## Rules
- Do not use the daemon as a generic privileged shell.
- Keep generated scheduler code and rollout commands reproducible from the workspace.
- State whether each conclusion is a fact, inference, or hypothesis.
- Prefer a single active scheduler experiment at a time so comparisons stay attributable.
- If the sched-ext ABI or helper surface looks version-sensitive, say so and tie the conclusion to the tested kernel version.
- Do not compare free-form notes alone when the same run can be compared through saved artifacts.

## Reference Material
- `references/official-docs.md`
- `references/rollout-checklist.md`
