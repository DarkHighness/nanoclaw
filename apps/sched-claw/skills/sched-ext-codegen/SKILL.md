---
name: "sched-ext-codegen"
description: "Use when workload evidence has to be turned into concrete sched-ext code changes, reference scaffolds, policy levers, and buildable candidate state. Covers narrow code generation grounded in prior analysis without requiring the host to orchestrate codegen."
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
   - keep a short design note near the candidate source or artifact directory
   - `scripts/scaffold_design_brief.sh` is a good default when you want a durable bridge from evidence and analysis into a concrete edit brief
   - include policy levers, invariants, risks, fallback criteria, and code targets
3. Choose the narrowest implementation surface.
   - reuse an existing candidate directory when the change is iterative
   - use `scripts/scaffold_sched_ext_candidate.sh` when you need a fresh candidate directory, build stub, or notes file
   - optional reference scaffolds live in `apps/sched-claw/templates/sched_ext/*.tmpl`; inspect them with normal file tools instead of assuming a host command must materialize them
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
   - build with normal shell commands or the scaffolded `build.sh`
   - verifier or libbpf failures are part of the design loop, not an afterthought
6. Keep code generation coupled to an evidence loop, not to a fixed host workflow.
   - use ordinary collection or analysis scripts when the bottleneck is still ambiguous
   - use the daemon `collect_perf` action only when attach-style privileged PMU capture is the missing evidence
   - avoid teaching the host one canonical scoring or mutation loop; leave that policy in the design note and the active skill path

## Rules
- Do not generate sched-ext code without stating which evidence and analysis records justify it.
- Do not mix unrelated policy changes into one candidate.
- Keep rollback criteria explicit when the new scheduler trades latency against throughput or fairness.

## Output checklist
- candidate path or id
- evidence and analysis references
- policy levers
- code targets
- invariants and risks
- build command or next build action

## Reference Material
- `references/codegen-levers.md`
- `../sched-ext-build-verify/references/build-and-verifier-checklist.md`

## Optional Helper Script
- `scripts/scaffold_sched_ext_candidate.sh`
  - creates a candidate source file, build stub, and notes file
  - can optionally seed the source from an existing template file
- `scripts/scaffold_design_brief.sh`
  - creates a design brief that links evidence refs, analysis refs, levers, invariants, risks, and fallback criteria
  - keeps the evidence-to-code bridge durable without moving that policy into the host
