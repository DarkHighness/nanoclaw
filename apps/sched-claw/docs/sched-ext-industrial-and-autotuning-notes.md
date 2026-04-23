# sched-ext Industrial and Autotuning Notes

This note captures the external references that most directly shape
`sched-claw` host design.

## 1. sched_ext host constraints from kernel docs

The Linux kernel `sched_ext` documentation establishes the non-negotiable
runtime boundary:

- BPF schedulers can be turned on and off dynamically.
- System integrity is preserved by reverting to fair-class scheduling on
  internal errors, stalled runnable tasks, or explicit SysRq fallback.
- The scheduler surface is expressed through `sched_ext_ops`, CPU selection,
  enqueue/dispatch hooks, and DSQ topology instead of a single monolithic
  scheduler callback.
- The ABI is intentionally unstable across kernel versions, so generated code
  must stay narrow and verified against the current host.

Design implication for `sched-claw`:

- keep privileged rollout behind a dedicated daemon with bounded leases
- keep build/verifier evidence durable because verifier failure is part of the
  normal design loop
- keep code generation focused on explicit levers such as wakeup placement,
  DSQ structure, slice policy, and cgroup isolation rather than "rewrite the
  whole scheduler"

Primary source:

- Linux kernel docs, `sched_ext`: <https://docs.kernel.org/scheduler/sched-ext.html>

## 2. Industrial loader patterns from the sched-ext ecosystem

The split between `scx_loader` and `scxctl` shows a production-oriented control
shape:

- a long-lived privileged daemon owns scheduler lifecycle
- operator or higher-level agents change scheduler, mode, and arguments through
  a narrower control plane
- runtime modes such as `Auto`, `Gaming`, `LowLatency`, `PowerSave`, and
  `Server` encode operator intent without exposing raw kernel privileges

The `sched-ext/scx` tree also shows that real schedulers are not blank-page
designs. They cluster around reusable ideas:

- latency-aware virtual deadlines (`scx_lavd`)
- cgroup-aware flattening and isolation (`scx_flatcg`)
- topology-aware or LLC-aware dispatch structure
- explicit statistics and loader-side configuration around the BPF core

Design implication for `sched-claw`:

- keep the daemon narrow and lifecycle-oriented
- keep templates and candidate mutations structured
- treat industrial scheduler implementations as evidence for useful levers, not
  as copy-paste targets

Primary sources:

- `scx_loader`: <https://github.com/sched-ext/scx-loader>
- `sched-ext/scx`: <https://github.com/sched-ext/scx>

## 3. Autotuning system lessons

OpenTuner and Google Vizier converge on several system-design lessons:

- the tuned system is often easier to experiment with than to model exactly
- no single search technique dominates across all search spaces
- search should be budgeted and durable
- noisy measurements must not be treated as hard truth from one run

Design implication for `sched-claw`:

- keep search policy, lineage, and decisions in the manifest
- avoid a single lucky run deciding promotion
- prefer explicit trial budgets and evidence gates over open-ended transcript
  memory

Primary sources:

- OpenTuner project and paper landing pages:
  - <https://opentuner.org/>
  - <https://www.csail.mit.edu/research/opentuner-extensible-framework-program-autotuning>
- Google Vizier paper landing page:
  - <https://research.google/pubs/google-vizier-a-service-for-black-box-optimization/>

## 4. Performance collection and analysis lessons

`perf stat` gives the right low-overhead substrate for first-pass workload
evidence because it supports:

- running a command directly
- attaching to existing PIDs
- cgroup-scoped collection
- machine-readable field-separated output
- bounded observation windows with `--timeout`

Top-down analysis adds an important constraint for interpretation:

- IPC/CPI and top-down slots are proxy indicators
- they help when direct throughput or latency is absent, but they do not
  replace direct service-level metrics when those exist

Design implication for `sched-claw`:

- make low-overhead PMU collection part of the durable experiment substrate
- auto-capture `perf stat` evidence where possible
- keep the measurement basis explicit as `direct` vs `proxy_estimate`
- only escalate to higher-overhead traces when the proxy layer cannot
  distinguish hypotheses

Primary sources:

- `perf-stat(1)`: <https://man7.org/linux/man-pages/man1/perf-stat.1.html>
- "A Top-Down Method for Performance Analysis and Counters Architecture":
  - <https://ieeexplore.ieee.org/document/6844459>
  - mirror summary page: <https://cris.haifa.ac.il/en/publications/a-top-down-method-for-performance-analysis-and-counters-architect/>

## 5. What this means for the host

The host should keep implementing these substrate properties:

- manifest-backed workload, collection, evaluation, and search policy
- automatic artifact capture for build, verifier, run, and low-overhead PMU
  evidence
- explicit distinction between direct workload success metrics and proxy CPU
  metrics
- code generation around named sched-ext levers, not unconstrained blank-page
  synthesis
- privileged rollout through a lifecycle daemon, not generic root shell access
