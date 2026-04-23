---
name: "sched-ext-run-evaluation"
description: "Use when baseline and candidate runs need to be executed and compared with reproducible artifacts instead of free-form notes. Covers repeated runs, metrics import, noise handling, and script-driven comparison instead of host-owned scoring."
aliases:
  - "run-evaluation"
tags:
  - "sched-ext"
  - "evaluation"
  - "scoring"
---

# sched-ext Run Evaluation

## When to use
- A workload contract is ready and the next step is to measure CFS versus a candidate.
- A candidate run completed and now needs interpretation.
- Results are noisy and the next action depends on whether the score is trustworthy.

## Read before acting
- `references/repetition-and-scoring.md`

## Workflow
1. Establish the baseline first.
   - run at least one clean CFS baseline with the same launcher and metrics file contract
2. Run candidates through the same launcher contract.
   - keep workload logs, metrics files, and daemon logs side by side under a stable artifact directory
   - if you need a repetitive wrapper, add or reuse a local script instead of expecting the host to own the loop
3. Prefer repeated runs when variance is visible.
   - repeat the same launcher command with explicit labels instead of changing the method between runs
   - if results drift across runs, say so and lower confidence
   - do not promote from a single noisy run just because one number improved
   - when you need richer trial analysis, use `scripts/compare_trials.py` and pick the reducer and optional outlier method that fit the workload; do not assume one technique is globally correct
4. Use explicit comparison artifacts.
   - reduce the primary metric and guardrails with the chosen reducer
   - keep the decision note next to the comparison output
   - check guardrails before celebrating the primary metric
5. End with a factual comparison.
   - what changed
   - whether the change is attributable
   - what still needs confirmation

## Rules
- Do not compare candidate notes to baseline notes when typed metrics exist.
- Do not hide missing runs or missing metrics.
- Treat noisy results as incomplete evidence, not as proof.

## Output checklist
- baseline run count
- candidate run count
- primary metric delta
- guardrail status
- confidence level and next action

## Reference Material
- `references/repetition-and-scoring.md`

## Optional Helper Script
- `scripts/compare_trials.py`
  - compares baseline and candidate runs from explicit result files or a manifest-like sidecar when such a file exists
  - supports caller-selected reducers and optional outlier methods
  - does not write back host policy; it is only an analysis aid
