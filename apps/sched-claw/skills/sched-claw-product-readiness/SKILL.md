---
name: "sched-claw-product-readiness"
description: "Use when the task is to decide whether a host is ready to run sched-claw as a real autotuning product. Covers readiness inspection, dependency gaps, daemon reachability, demo assets, and how to turn missing prerequisites into an explicit operator plan."
aliases:
  - "product-readiness"
tags:
  - "sched-claw"
  - "product"
  - "readiness"
---

# sched-claw Product Readiness

## When to use
- Decide whether the current host can support sched-claw end to end.
- Explain why an agent run, build, verifier probe, daemon rollout, or demo launch is blocked.
- Stage a system before starting real autotune work.

## Read before acting
- `references/readiness-matrix.md`
- `references/harness-engineering.md`

## Workflow
1. Start from the requested capability, not a generic checklist.
   - Distinguish analysis-only, build-and-verify, privileged rollout, and demo execution.
   - State which one is blocked.
2. Run the host readiness surface first.
   - Prefer `sched-claw doctor --style plain` or `--style table`.
   - Treat the output as the product-facing readiness summary, not as a model-only heuristic.
3. Classify failures by layer.
   - runtime: provider credentials and model bootstrap
   - skills: builtin skill bundle, helper scripts, and shared Linux perf skills
   - toolchain: `clang`, `bpftool`, `perf`, `uv`, `python3`
   - kernel: BTF and cgroup support
   - daemon: privileged rollout reachability
   - demo: LLVM or MySQL launcher prerequisites
4. Keep the harness split clean.
   - normal tools plus repo scripts handle collection, analysis, plotting, and code generation
   - the daemon handles privileged sched-ext lifecycle only
   - if a task needs deterministic automation, prefer an explicit script over a new host workflow command
5. Convert gaps into an operator plan.
   - State which failures block all progress and which only block a specific demo or rollout phase.
   - Keep remediation explicit and minimal.
6. Re-run readiness after the operator changes the host.
   - Do not assume a missing dependency is fixed until `sched-claw doctor` says so.

## Rules
- Do not invent hidden prerequisites. Use the doctor surface and the real launcher scripts.
- Do not claim the product is ready when build, daemon, or provider bootstrap is blocked.
- Keep optional demo gaps separate from core autotune gaps.

## Output checklist
- current readiness summary
- blocking gaps vs optional gaps
- exact remediation steps
- the next command to rerun for confirmation

## Reference Material
- `references/readiness-matrix.md`
- `references/harness-engineering.md`
