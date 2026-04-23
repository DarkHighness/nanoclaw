---
name: "sched-policy-mapping"
description: "Use when evidence has to be turned into explicit scheduler policy levers, invariants, and measurement hypotheses before code generation or rollout. Keeps policy reasoning reusable across workloads and candidate implementations."
aliases:
  - "policy-mapping"
  - "lever-mapping"
tags:
  - "scheduler"
  - "policy"
  - "analysis"
---

# sched Policy Mapping

## When to use
- Evidence exists, but the scheduler implication is still only implicit.
- Multiple candidate levers are possible and need to be narrowed before code edits.
- A workload profile or demo needs a durable policy note without committing to one implementation shape yet.

## Read before acting
- `references/lever-mapping.md`

## Inputs
- evidence refs or artifact paths
- analysis refs or ranked findings
- explicit workload objective and guardrails

## Outputs
- policy note or mapping brief
- explicit levers
- invariants
- measurement questions for the next capture or rollout

## Composition
- Pair with `sched-workload-contract` when the objective or guardrail is still vague.
- Pair with `sched-perf-analysis` or `linux-scheduler-triage` when the evidence still needs interpretation.
- Pair with `sched-ext-codegen` when the next step is source editing.
- Pair with `sched-ext-run-evaluation` or `sched-ext-rollout-safety` when the policy still needs a verification plan.

## Method
1. Start from the objective, not the implementation.
   - latency, throughput, fairness, locality, tail control, or isolation
2. Map evidence to the smallest plausible policy levers.
   - wakeup CPU choice
   - DSQ topology
   - slice sizing
   - migration control
   - class or cgroup isolation
3. Keep invariants separate from hypotheses.
   - invariant: what must remain true if the candidate is valid
   - hypothesis: what should improve if the chosen lever is correct
4. Keep follow-up questions explicit.
   - what capture would falsify the lever choice
   - what rollout guard would invalidate the policy
5. Persist the result in a durable note.
   - `scripts/scaffold_policy_mapping.sh` is the default helper when you want a reusable mapping note

## Rules
- Do not jump from evidence straight to code without an explicit lever mapping when multiple scheduler explanations are still plausible.
- Do not collapse objective, lever, invariant, and rollout guard into one vague summary.
- Keep the note implementation-agnostic until a specific candidate path is chosen.

## Output checklist
- evidence refs
- analysis refs
- objective
- policy levers
- invariants
- open measurement questions
- rollback or invalidation conditions

## Reference Material
- `references/lever-mapping.md`

## Optional Helper Script
- `scripts/scaffold_policy_mapping.sh`
  - creates a reusable policy note with evidence refs, objective, levers, invariants, open questions, and invalidation conditions
  - keeps evidence-to-policy reasoning durable before code-specific scaffolding begins
