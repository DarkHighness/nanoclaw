---
name: "sched-ext-build-verify"
description: "Use when sched-ext source has to be materialized or edited, then compiled, verifier-checked, and persisted with concrete artifacts. Covers build command hygiene, libbpf or verifier triage, and experiment build records."
aliases:
  - "build-verify"
tags:
  - "sched-ext"
  - "build"
  - "verifier"
---

# sched-ext Build Verify

## When to use
- A candidate has source code but no verified object yet.
- A verifier failure or libbpf load error needs to be turned into an actionable diagnosis.
- A template was materialized and now needs a durable build record.

## Read before acting
- `references/build-and-verifier-checklist.md`

## Workflow
1. Make sure the candidate spec is durable first.
   - keep `source_path`, `object_path`, `build_command`, daemon argv, and knobs in the candidate record
2. Build through the experiment substrate when possible.
   - `sched-claw experiment build <experiment> --candidate-id <id>`
   - this captures compiler stdout/stderr and verifier logs in one artifact tree
3. Read the build result before changing code.
   - separate compiler failure, missing include/toolchain failure, and verifier rejection
4. For verifier failures, narrow the cause.
   - helper or kfunc availability
   - map layout or BTF assumptions
   - invalid pointer or bounds logic
   - unsupported struct_ops expectations for the current kernel
5. Keep the next fix scoped.
   - fix the minimum code or build assumption needed
   - rebuild and keep the artifact trail

## Rules
- Do not trust comments or intentions over the captured verifier logs.
- Do not activate a candidate whose latest build or verifier result failed unless the operator explicitly overrides the gate.
- Keep the object path explicit so deploy and run surfaces reuse the same artifact.

## Output checklist
- build command used
- compiler result
- verifier backend and result
- artifact paths for stdout, stderr, and verify logs
- next code or environment fix

## Reference Material
- `references/build-and-verifier-checklist.md`
