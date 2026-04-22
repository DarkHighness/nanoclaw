# sched-ext Design References

## Official kernel docs
- sched-ext overview and switching model: https://docs.kernel.org/scheduler/sched-ext.html
- CFS design reference: https://docs.kernel.org/6.6/scheduler/sched-design-CFS.html
- scheduler domains: https://docs.kernel.org/scheduler/sched-domains.html
- PSI for rollout guardrails: https://docs.kernel.org/6.10/accounting/psi.html

## What to read first
1. `sched-ext` for lifecycle, DSQ behavior, and example schedulers under `tools/sched_ext/`.
2. CFS design for the baseline semantics you are replacing.
3. scheduler domains when CPU selection or balancing behavior appears topology-sensitive.
4. PSI when you need rollout guardrails tied to actual stall behavior.

## Caveat
- The sched-ext docs explicitly note ABI instability between kernel versions. Treat every policy and helper assumption as kernel-version-scoped unless you have verified it on the target host.
