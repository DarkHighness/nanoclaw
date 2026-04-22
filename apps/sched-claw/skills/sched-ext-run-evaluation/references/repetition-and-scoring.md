# Repetition and Scoring

## Minimal discipline
- one clean baseline before any candidate score
- one clean candidate run before reading score output

## When to repeat
- unstable launcher timing
- noisy multi-tenant host
- daemon startup or stop behavior varies between runs
- direct metrics contradict proxy metrics

## Interpretation rule
- `promote` means the current typed evidence supports keeping the candidate for the next step.
- `revise` means the candidate is plausible but not good enough yet.
- `blocked` means guardrails or rollout gates failed.
- `incomplete` means the evidence is insufficient.
