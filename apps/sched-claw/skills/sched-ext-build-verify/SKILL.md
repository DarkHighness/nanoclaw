---
name: "sched-ext-build-verify"
description: "Use when sched-ext source has to be edited, then compiled, verifier-checked, and persisted with concrete artifacts. Covers build command hygiene, libbpf or verifier triage, and script-friendly artifact capture."
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
1. Make sure the candidate source and artifact directory are durable first.
   - keep `source`, `object`, `build.sh`, and notes near each other in the workspace
2. Build through normal shell commands or the scaffolded `build.sh`.
   - capture compiler stdout or stderr and verifier logs into files next to the source or artifact directory
   - `scripts/capture_build_verifier_artifacts.sh` is the default helper when you want one command to leave durable build, verify, and status files behind
   - `scripts/summarize_build_verifier.py` is the default helper when artifacts already exist and the next question is how to classify the failure mode without relying on transcript-only notes
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
- Keep the object path explicit so daemon activation reuses the same artifact.

## Output checklist
- build command used
- compiler result
- verifier backend and result
- artifact paths for stdout, stderr, and verify logs
- next code or environment fix

## Reference Material
- `references/build-and-verifier-checklist.md`

## Optional Helper Scripts
- `scripts/capture_build_verifier_artifacts.sh`
  - captures build command, stdout, stderr, and exit status into a durable artifact directory
  - optionally captures verifier command, stdout, stderr, and exit status as well
  - preferred when code edits are done and the next question is whether the object and verifier evidence are durable enough for rollout discussion
- `scripts/summarize_build_verifier.py`
  - classifies captured build or verifier artifacts into common failure categories such as missing toolchain input, verifier rejection, or BTF mismatch
  - can emit Markdown or JSON if the next step needs a durable diagnosis note
