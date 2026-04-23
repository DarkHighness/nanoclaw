---
name: "sched-ext-run-evaluation"
description: "Use when baseline and candidate runs need to be executed and compared with reproducible artifacts instead of free-form notes. Covers repeated runs, metrics import, noise handling, and score interpretation."
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
2. Run candidates through the experiment substrate.
   - `sched-claw experiment run <experiment> ...`
   - let the substrate capture workload logs, metrics, and daemon logs
3. Prefer repeated runs when variance is visible.
   - use `sched-claw experiment run <experiment> --repeat <N> ...` instead of copying the same command by hand
   - if results drift across runs, say so and lower confidence
   - do not promote from a single noisy run just because one number improved
4. Use `sched-claw experiment score`.
   - interpret the typed decision: `promote`, `revise`, `blocked`, `incomplete`
   - read the improving-run ratio and primary outlier count, not just the median delta
   - check guardrails before celebrating the primary metric
   - if the score led to a durable keep or stop decision, persist it with `sched-claw experiment record-decision ...`
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
- improving-run ratio
- primary outlier count
- guardrail status
- confidence level and next action

## Reference Material
- `references/repetition-and-scoring.md`
