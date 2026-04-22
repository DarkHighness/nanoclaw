# Readiness Matrix

## Core readiness
- Provider credentials for the selected model provider
- `clang`
- `bpftool`
- `/sys/kernel/btf/vmlinux`
- reachable `sched-claw-daemon`
- builtin sched-claw skills

## Strongly recommended
- `perf`
- cgroup v2 controllers
- shared `apps/code-agent/skills` root

## Demo-specific
- LLVM demo: `cmake`, `ninja`
- MySQL demo: `sysbench`, `docker` in default mode

## Operator rule
- A missing core prerequisite blocks real end-to-end autotune work.
- A missing demo prerequisite blocks that demo only.
