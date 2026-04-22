# sched-claw

`sched-claw` is a thin host built on the same substrate as `code-agent`, but it
removes the heavy management and TUI surfaces and narrows the runtime around one
job: investigate Linux scheduling behavior, generate a new `sched-ext`
scheduler, and hand privileged rollout work to a dedicated daemon.

The host intentionally does not hard-code a scheduler workflow. Skills define
the SOP; the host only provides generic local capabilities for experiment state,
template materialization, scoring, and privileged rollout.

## Design constraints

- Keep the host thin. Reuse `core`, `runtime`, `config`, and `store` instead of
  duplicating `code-agent`'s larger boot pipeline.
- Keep the tool surface explicit and small.
  - file/code editing: `read`, `write`, `edit`, `patch_files`, `glob`, `grep`,
    `list`
  - shell execution: `exec_command`, `write_stdin`
  - live documentation lookup: `web_search`, `web_fetch`
  - skill discovery: `skills_list`, `skill_view`, `tool_discover`
  - privileged sched-ext lifecycle: `sched_ext_daemon`
- Do not add a dedicated performance-collection tool. Collection and analysis
  stay in reusable skills plus the existing shell/file/web surfaces.
- Do not let the agent spawn arbitrary root commands. Privileged launch and
  rollback go through the daemon over a local Unix socket, with path and process
  constraints enforced server-side.

## Runtime shape

- `sched-claw` provides:
  - one-shot execution: `sched-claw exec "prompt"`
  - a simple line REPL: `sched-claw repl`
  - a local template catalog for sched-ext source scaffolding:
    - `sched-claw template list --style table`
    - `sched-claw template show latency_guard --style plain`
  - a local experiment substrate for workload contracts, baselines, candidates, and scoring:
    - `sched-claw experiment list --style table`
    - `sched-claw experiment init --id demo --workload-name bench --primary-metric latency_ms --primary-goal minimize`
    - `sched-claw experiment init --id demo --workload-name bench --primary-metric latency_ms --primary-goal minimize --min-baseline-runs 3 --min-candidate-runs 3 --min-primary-improvement-pct 2 --max-primary-relative-spread-pct 10`
    - `sched-claw experiment init --id demo --workload-name app --target-pid 4242 --primary-metric ipc --primary-goal maximize --performance-basis proxy_estimate --proxy-metric ipc:maximize --proxy-metric cpi:minimize`
    - `sched-claw experiment init --id demo --workload-name service --target-cgroup /sys/fs/cgroup/work.slice --primary-metric latency_ms --primary-goal minimize --guardrail throughput:maximize:5`
    - `sched-claw experiment set-evaluation-policy demo --min-baseline-runs 5 --min-candidate-runs 5 --max-primary-relative-spread-pct 8`
    - `sched-claw experiment add-candidate demo --candidate-id locality-v1 --template dsq_local`
    - `sched-claw experiment set-candidate demo --candidate-id locality-v1 --template dsq_local --daemon-arg loader --daemon-arg {source}`
    - `sched-claw experiment materialize demo --candidate-id locality-v1 --template dsq_locality --loader ./loader --loader-arg {source}`
    - `sched-claw experiment build demo --candidate-id locality-v1 --style table`
    - `sched-claw experiment run demo --label cfs-a --style table`
    - `sched-claw experiment run demo --candidate-id locality-v1 --label cand-a --timeout-seconds 60 --lease-seconds 60 --style table`
    - `sched-claw experiment record-baseline demo --label cfs-baseline --artifact-dir artifacts/baseline --metric latency_ms=12.4`
    - `sched-claw experiment record-candidate demo --candidate-id locality-v1 --label run-a --artifact-dir artifacts/cand-a --metric latency_ms=9.1`
    - `sched-claw experiment score demo --style table`
    - `sched-claw experiment deploy demo --candidate-id locality-v1 --lease-seconds 300 --style table`
  - a product-facing readiness surface:
    - `sched-claw doctor --style table`
    - `sched-claw doctor --style plain`
  - local inspection and audit helpers such as:
    - `sched-claw tool list --style table`
    - `sched-claw tool show sched_ext_daemon --style plain`
    - `sched-claw skill list --style table`
    - `sched-claw skill show linux-scheduler-triage --style plain`
    - `sched-claw sessions --style table`
    - `sched-claw sessions "wakeup latency" --style plain`
    - `sched-claw session last --style plain`
    - `sched-claw export-transcript last artifacts/session.txt`
    - `sched-claw export-events last artifacts/session.jsonl`
    - `sched-claw resume last "continue from the prior analysis"`
    - `sched-claw daemon status --style table`
- `sched-claw-daemon` is a separate binary intended to run with elevated
  privileges. It manages one active sched-ext deployment at a time, captures the
  child process logs, and exposes:
  - `status`
  - `activate`
  - `stop`
  - `logs`

## Skills

Built-in skills are materialized under:

- `.nanoclaw/apps/sched-claw/builtin-skills`

They cover:

- product readiness and operator dependency checks
- workload contract definition and metric policy
- Linux scheduler-focused evidence collection and triage
- sched-ext build, verifier, run, score, and rollout safety loops
- Translating workload evidence into sched-ext policy and rollout steps
- LLVM/clang build autotune demos with direct wall-clock metrics plus IPC/CPI fallback
- MySQL sysbench autotune demos with direct throughput and latency metrics

When the repository also contains [`apps/code-agent/skills`](./../code-agent/skills),
`sched-claw` loads that skill root by default as well. This lets the agent reuse
existing Linux performance collection, analysis, and eBPF engineering skills
without inflating the model-visible tool surface.

The model is expected to load those skills with `skill_view` before inventing
new measurement recipes.

Each builtin skill now ships with reference material under its own
`references/` directory so the host and operator can inspect the workflow
outside the model runtime as well.

## Local inspection UI

Even without a TUI, `sched-claw` now exposes two consistent operator-facing
output styles inspired by the management surfaces in `code-agent`:

- `--style table`
  - default for local inspection commands
  - optimized for quick scanning of tools, skills, daemon status, and logs
- `--style plain`
  - optimized for piping, logs, and copying into notes or issue trackers

The style switch applies to:

- `sched-claw doctor`
- `sched-claw template list`
- `sched-claw template show <name>`
- `sched-claw experiment list`
- `sched-claw experiment show <id>`
- `sched-claw experiment score <id>`
- `sched-claw sessions [query]`
- `sched-claw session <id>`
- `sched-claw tool list`
- `sched-claw tool show <name>`
- `sched-claw skill list`
- `sched-claw skill show <name>`
- `sched-claw daemon status`
- `sched-claw daemon activate ...`
- `sched-claw daemon logs`
- `sched-claw daemon stop`

The REPL also supports local inspection commands:

- `:format <table|plain>`
- `:doctor`
- `:experiments`
- `:experiment <id>`
- `:score <id>`
- `:templates`
- `:template <name>`
- `:tools`
- `:tool <name>`
- `:skills`
- `:skill <name>`
- `:sessions [query]`
- `:session <id>`
- `:resume <id>`
- `:daemon status`
- `:daemon logs [N]`

## Session history and resume

Persistent session history is a first-class operator surface now, not an
implicit side effect hidden in the store directory.

- `sched-claw sessions`
  - list persisted sessions in recency order
- `sched-claw sessions "query text"`
  - search prompts, transcript text, and indexed event metadata
- `sched-claw session <id|last>`
  - inspect one persisted session with transcript and token usage details
- `sched-claw export-transcript <id|last> <path>`
  - write an operator-readable transcript export
- `sched-claw export-events <id|last> <path>`
  - write raw JSONL event history for audit or post-processing
- `sched-claw resume <id|last> [prompt...]`
  - fork a fresh live agent session from a persisted runtime snapshot and
    optionally continue with a one-shot prompt immediately

These commands intentionally reuse the shared `store` and `runtime` layers
instead of maintaining a separate history protocol.

## Experiment substrate

Workload-driven sched-ext tuning should not rely on transcript prose alone.
`sched-claw` now includes a local experiment manifest and scoring layer under:

- `.nanoclaw/apps/sched-claw/experiments/<id>/experiment.toml`

The substrate is generic on purpose. Typical commands include:

- `experiment init`
  - define the workload contract, target selector, primary metric, performance policy, and guardrails
- `experiment set-evaluation-policy`
  - tighten or relax the evidence gate after an experiment already exists
  - keep minimum run counts, minimum improvement thresholds, and primary-metric spread limits durable in the manifest
- `experiment add-candidate` / `experiment set-candidate`
  - persist candidate metadata, source/object paths, daemon argv, build commands, and knobs
- `experiment materialize`
  - turn a named sched-ext template plus knob values into concrete source under the experiment directory
- `experiment build`
  - execute the candidate build command from the workspace root
  - capture build stdout/stderr, exit code, and a short failure summary under the experiment artifact tree
  - run a verifier probe through `bpftool -d -L prog loadall` by default so libbpf and verifier logs are captured without pinning persistent bpffs state
  - persist the build and verifier records back into the candidate manifest entry
- `experiment run`
  - execute the script workload contract and capture stdout/stderr, metrics, and artifact paths under the experiment artifact tree
  - for candidate runs, activate the sched-ext loader through the daemon, stop it after the workload finishes, and persist daemon logs next to the run artifacts
  - refuse candidate rollout by default when the latest build or verifier record is not successful; use `--allow-unverified-build` only as an explicit override
- `experiment record-baseline`
  - store one or more CFS baseline runs with artifact paths and measured metrics
- `experiment record-candidate`
  - store one or more sched-ext runs for a specific candidate
- `experiment score`
  - compare candidate medians against the baseline and classify each candidate as `promote`, `revise`, `blocked`, or `incomplete`
  - also report the current evaluation policy, baseline spread, candidate spread, and any reasons that kept a candidate from promotion
- `experiment deploy`
  - activate a chosen candidate through the daemon and persist the deployment record back into the manifest
  - optional `--lease-seconds` bounds how long the privileged deployment may remain active if the client dies or forgets to stop it

This is intentionally a host-local substrate, not a new model-visible tool.
Agents are expected to call these commands through the existing shell tool so
the visible tool surface stays minimal.

Which commands to use, and in what order, is not host policy. The active skill
SOP should decide the loop; the host only makes the state durable and reusable.

Materialized candidates now also persist both `source_path` and `object_path`,
so the build and rollout layers share one concrete artifact contract instead of
re-deriving object names ad hoc.

## Evaluation Policy

Every experiment manifest now carries an explicit evaluation policy alongside
its primary metric and guardrails. This keeps promotion criteria durable instead
of implicit:

- minimum baseline run count
- minimum candidate run count
- optional minimum primary-metric improvement percent
- optional maximum primary-metric relative spread percent

If the manifest says the evidence is insufficient, `sched-claw experiment score`
will keep the candidate `incomplete` even when a single run looked promising.

## Workload selectors and performance policy

`sched-claw` can now record the workload target explicitly instead of leaving it
as free text. `experiment init` supports:

- script target
  - default when you use `--workload-cwd`, `--workload-arg`, or `--workload-env`
- `--target-pid <pid>`
- `--target-uid <uid>`
- `--target-gid <gid>`
- `--target-cgroup <path>`

Only one non-script selector may be set at a time. Script launch fields do not
mix with pid/uid/gid/cgroup selectors.

Performance intent is also stored explicitly:

- direct metrics
  - use throughput and latency when they are available
  - express the priority with `--primary-metric`, `--primary-goal`, and optional guardrails
- proxy estimate
  - use `--performance-basis proxy_estimate`
  - record proxies such as `IPC` or `CPI` with `--proxy-metric ipc:maximize` and `--proxy-metric cpi:minimize`

The host stores this as manifest metadata. It does not force the agent to use a
fixed evaluation workflow, but it makes the basis of a decision auditable.

## Template catalog

The local sched-ext template catalog lives under:

- `apps/sched-claw/templates/sched_ext/`

Current built-in starting points are:

- `dsq_locality`
  - locality-biased wakeup and migration controls
- `latency_guard`
  - short-slice, wakeup-sensitive interactive controls
- `balanced_queue`
  - shared-queue throughput controls
- `cgroup_lane`
  - cgroup-aware class and lane controls

`experiment materialize` writes concrete `.bpf.c` source files under the
experiment state directory by default and records the resulting source path,
build command, knobs, and optional daemon argv back into the candidate spec.

## Doctor

`sched-claw doctor` is the operator-facing readiness surface for the host. It
does not change the model-visible tool surface; it checks whether the current
workspace and machine are ready for real sched-claw use.

Current checks include:

- selected provider credentials for the active primary model
- builtin sched-claw skills and shared `apps/code-agent/skills` availability
- sched-ext template catalog presence
- privileged daemon socket reachability
- core toolchain availability such as `clang`, `bpftool`, and `perf`
- kernel prerequisites such as BTF and cgroup v2
- demo scripts plus LLVM and MySQL demo prerequisites

Use it before claiming the product is ready, and after an operator changes the
host to clear a blocking gap.

## Demo scripts

For operator demos, `sched-claw` now ships standalone scripts that call the host
instead of adding a dedicated workload command surface.

- LLVM/clang build autotune
  - demo wrapper:
    - `apps/sched-claw/scripts/demos/llvm-clang-autotune.sh`
  - workload launcher:
    - `apps/sched-claw/scripts/workloads/run-llvm-clang-build.sh`
  - builtin skill:
    - `llvm-clang-build-tuning`
- MySQL sysbench autotune
  - demo wrapper:
    - `apps/sched-claw/scripts/demos/mysql-sysbench-autotune.sh`
  - workload launcher:
    - `apps/sched-claw/scripts/workloads/run-mysql-sysbench.sh`
  - builtin skill:
    - `mysql-sysbench-tuning`

The wrapper scripts do two things:

- initialize a structured experiment manifest with the right direct metrics,
  guardrails, proxy hints, and replayable launcher argv
- call `sched-claw exec` with a prompt that points the agent at the workload-
  specific skill instead of hard-coding a host workflow

Typical dry-run examples:

```bash
tmp=$(mktemp -d)
mkdir -p "$tmp/llvm"
printf 'cmake_minimum_required(VERSION 3.20)\n' >"$tmp/llvm/CMakeLists.txt"
apps/sched-claw/scripts/demos/llvm-clang-autotune.sh --llvm-src "$tmp" --dry-run
```

```bash
apps/sched-claw/scripts/demos/mysql-sysbench-autotune.sh --dry-run
```

The workload launchers are also usable directly when you want reproducible local
measurements without invoking the model runtime.

## Daemon startup

Start the daemon from the workspace root:

```bash
sudo cargo run --manifest-path apps/Cargo.toml -p sched-claw --bin sched-claw-daemon -- serve
```

For repeatable local startup without retyping the client uid/gid handoff flags,
use:

```bash
apps/sched-claw/scripts/start-root-daemon.sh
```

That script builds the daemon as the current user, re-execs it under `sudo`,
and passes `--client-uid/--client-gid` automatically so the socket becomes
usable from the non-root `sched-claw` client.

For agent execution, provider credentials are still loaded from the workspace
root `.env` or the parent shell environment. The file
[`apps/sched-claw/.env.example`](./.env.example) is only a template and should
be copied or mirrored to the workspace root before running `sched-claw exec`.

By default the daemon listens on:

- `.nanoclaw/apps/sched-claw/sched-claw.sock`

When launched through `sudo`, the daemon will try to hand the socket back to the
original invoking user via `SUDO_UID`/`SUDO_GID` so the unprivileged agent
process can connect without exposing the socket globally.

## Why the daemon exists

`sched-ext` deployment is the only operation here that should cross the normal
workspace-write sandbox boundary. Everything else stays on the standard
substrate path:

- evidence capture and artifact management use normal tools
- scheduler source generation uses normal file-edit tools
- only privileged activation, stop, and log inspection use `sched_ext_daemon`

The daemon now also supports bounded leases for active deployments. That gives
the host a generic safety primitive for rollout windows without turning the
daemon into a workflow engine.

That split preserves one clear trust boundary instead of scattering privilege
across ad hoc shell commands.

## Validation

Automated validation currently includes:

- `cargo test --manifest-path apps/Cargo.toml -p sched-claw`
  - unit tests for builtin skill materialization and daemon launch validation
  - integration tests that spawn the real `sched-claw-daemon` binary, activate a
    mock loader, verify `status/logs/stop`, and verify automatic reaping after a
    child exits on its own
- `apps/sched-claw/scripts/smoke-daemon-e2e.sh`
  - manual protocol-level smoke check using the built binaries

Recent additions also have unit coverage for:

- experiment manifest persistence and guardrail scoring
- build and verifier capture for materialized sched-ext candidates
- workload run capture, metrics import, and rollout gating
- REPL command parsing
- session history reference resolution and transcript rendering
- startup catalog alias resolution
- table/plain rendering for tool, skill, session, and daemon inspection views
- demo script dry-run bootstrapping for LLVM/clang and MySQL sysbench

The root-required path still depends on a host that can actually run `sudo`
interactively or through a service manager. The repository now includes the
startup script for that path, but the repository test suite does not assume
passwordless root.
