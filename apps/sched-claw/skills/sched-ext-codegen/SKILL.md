---
name: "sched-ext-codegen"
description: "Use when workload evidence has to be turned into concrete sched-ext code changes, template choices, policy levers, and buildable candidate state. Covers design recording, template materialization, and narrow code generation grounded in prior analysis."
aliases:
  - "codegen"
  - "sched-codegen"
tags:
  - "sched-ext"
  - "ebpf"
  - "codegen"
---

# sched-ext Codegen

## When to use
- Analysis already points to specific scheduler levers.
- A new sched-ext candidate must be generated or revised.
- Template choice, code targets, and invariants need to be captured before build and rollout.

## Read before acting
- `references/codegen-levers.md`
- `../sched-ext-build-verify/references/build-and-verifier-checklist.md`

## Workflow
1. Start from durable evidence, not from a blank file.
   - identify the evidence ids and analysis ids that justify the code change
2. Persist the design intent before editing.
   - `sched-claw experiment record-design <experiment> --design-id ...`
   - include candidate id, policy levers, invariants, risks, fallback criteria, and code targets
3. Choose the narrowest implementation surface.
   - reuse an existing candidate when the change is iterative
   - when the change is a mutation of an earlier idea, prefer `sched-claw experiment fork-candidate ...` so lineage and mutation notes stay explicit
   - materialize a template only when it genuinely reduces boilerplate
4. Keep the code change scoped to the chosen levers.
   - wakeup placement
   - dispatch queue topology
   - slice budgeting
   - migration policy
   - workload isolation
   - when industrial references such as `scx_lavd` or cgroup schedulers are
     relevant, extract the narrow lever and rationale instead of cloning the
     whole scheduler shape
5. Move immediately into build and verifier capture.
   - `sched-claw experiment build <experiment> --candidate-id <id>`
   - verifier or libbpf failures are part of the design loop, not an afterthought

## Rules
- Do not generate sched-ext code without stating which evidence and analysis records justify it.
- Do not mix unrelated policy changes into one candidate.
- Keep rollback criteria explicit when the new scheduler trades latency against throughput or fairness.

## Output checklist
- candidate id
- evidence and analysis references
- policy levers
- code targets
- invariants and risks
- build command or next build action

## Reference Material
- `references/codegen-levers.md`
- `../sched-ext-build-verify/references/build-and-verifier-checklist.md`
