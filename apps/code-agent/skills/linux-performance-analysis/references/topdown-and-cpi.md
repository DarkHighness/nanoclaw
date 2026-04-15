# Top-down and CPI interpretation

This reference is grounded in:
- Intel VTune cookbook, "Top-Down Microarchitecture Analysis Method", https://www.intel.com/content/www/us/en/docs/vtune-profiler/cookbook/2023-1/top-down-microarchitecture-analysis-method.html
- `perf-stat(1)` manual, https://man7.org/linux/man-pages/man1/perf-stat.1.html
- perf wiki, https://perfwiki.github.io/main/top-down-analysis/
- Arm learning path example for Linux perf top-down commands, https://learn.arm.com/learning-paths/cross-platform/topdown-compare/2-code-examples/
- Brendan Gregg, "Linux perf Examples", https://www.brendangregg.com/perf.html
- Collabora, "Performance analysis in Linux", https://www.collabora.com/news-and-blog/blog/2017/03/21/performance-analysis-in-linux/

## Core formulas
- IPC = instructions / cycles
- CPI = cycles / instructions

These are throughput indicators, not root causes.

## Useful perf commands

```bash
perf stat --topdown -- <command>
perf stat -M topdownl1 -- <command>
perf stat -e cycles,instructions,branches,branch-misses,cache-references,cache-misses -- <command>
```

Use `taskset` or other CPU pinning if workload migration would distort the PMU view.

## Top-down Level 1 categories
- Retiring: useful work retired.
- Bad Speculation: branch mispredicts or machine clears wasted slots.
- Frontend Bound: fetch or decode undersupplies the backend.
- Backend Bound: execution units or memory cannot accept or complete work fast enough.

## Practical interpretation
- High Backend Bound plus cache or memory signals suggests memory latency, bandwidth, or execution-port pressure.
- High Frontend Bound suggests instruction fetch, decode, I-cache, ITLB, or branch resteer issues.
- High Bad Speculation suggests branch prediction or machine-clear waste.
- High Retiring with poor wall-clock performance may mean the algorithm is doing a lot of useful but excessive work.

## CPI breakdown in this skill

Use "CPI breakdown" to mean relating CPI or IPC changes to counter evidence such as:
- branch misses
- cache misses
- frontend or backend stall signals
- top-down categories

Do not invent precise subcomponents that the collected counters do not support.

## Caveats
- `perf stat --topdown` uses the full PMU and may need the NMI watchdog disabled for stable results on some systems.
- Multiplexed or phase-changing workloads can make top-down numbers misleading.
- Compare CPI or IPC only across comparable workloads, input sets, and microarchitectures.
