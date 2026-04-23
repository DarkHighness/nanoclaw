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
- visible `/proc/sys/kernel/perf_event_paranoid`
- cgroup v2 controllers
- shared `apps/code-agent/skills` root

## Demo-specific
- Demo requirements are separate from `sched-claw doctor`.
- LLVM demo: `cmake`, `ninja`
- MySQL demo: `docker` in default self-bootstrapped mode

## Operator rule
- A missing core prerequisite blocks real end-to-end autotune work.
- A missing demo prerequisite blocks that demo only.
