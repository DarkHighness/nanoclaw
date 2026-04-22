# LLVM/Clang Demo Contract

- Workload launcher:
  - `apps/sched-claw/scripts/workloads/run-llvm-clang-build.sh`
- Output artifacts:
  - `configure.log`
  - `build.log`
  - `metrics.env`
- Direct metric contract:
  - `build_seconds` is the main demo metric.
  - `configure_seconds` is auxiliary.
- Proxy fallback:
  - Use `ipc` and `cpi` only when direct build timing is not a trustworthy basis.
- Recommended target selector:
  - `script` for the built-in demo wrapper
  - `pid` only if the build was launched outside sched-claw and the operator wants to attach to an existing process
