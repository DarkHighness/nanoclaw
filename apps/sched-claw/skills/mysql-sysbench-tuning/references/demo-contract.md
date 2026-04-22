# MySQL Sysbench Demo Contract

- Workload launcher:
  - `apps/sched-claw/scripts/workloads/run-mysql-sysbench.sh`
- Output artifacts:
  - `prepare.log`
  - `warmup.log`
  - `run.log`
  - `cleanup.log`
  - `metrics.env`
- Direct metric contract:
  - `transactions_per_sec`
  - `queries_per_sec`
  - `p95_latency_ms`
  - `avg_latency_ms`
- Recommended target selector:
  - `script` for the built-in demo wrapper
  - `cgroup` when MySQL is already isolated in a service slice or container cgroup
  - `pid` only when attaching to an already-running benchmark or database process
