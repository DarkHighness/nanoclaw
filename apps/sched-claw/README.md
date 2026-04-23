# sched-claw

`sched-claw` is a thin host built on the same substrate as `code-agent`, but it
removes the heavy management and TUI surfaces and narrows the runtime around one
job: investigate Linux scheduling behavior, generate a new `sched-ext`
scheduler, and hand privileged rollout work to a dedicated daemon.

The host intentionally does not hard-code a scheduler workflow. Skills define
the SOP; the host primarily provides generic local capabilities for session
history, skill discovery, filesystem or shell access, and privileged rollout.

The primary direction is now a thinner harness: normal tools plus
repository-embedded skills and scripts for collection, analysis, plotting, and
code generation; a narrow privileged daemon for sched-ext lifecycle control.

## Design constraints

- Keep the host thin. Reuse `core`, `runtime`, `config`, and `store` instead of
  duplicating `code-agent`'s larger boot pipeline.
- Keep the tool surface explicit and small.
  - file/code editing: `read`, `write`, `edit`, `patch_files`, `glob`, `grep`,
    `list`
  - shell execution: `exec_command`, `write_stdin`
  - live documentation lookup: `web_search`, `web_fetch`
  - skill discovery: `skills_list`, `skill_view`, `tool_discover`
  - privileged sched-ext lifecycle and bounded perf capture: `sched_ext_daemon`
- Do not add a dedicated performance-collection tool. Collection and analysis
  stay in reusable skills plus the existing shell/file/web surfaces.
- Do not let the agent spawn arbitrary root commands. Privileged launch and
  rollback go through the daemon over a local Unix socket, with path and process
  constraints enforced server-side.

## Crate boundaries

The supported `sched-claw` build now keeps only the crates that are on the
primary runtime path:

- `apps/sched-claw/crates/daemon-protocol`
  - owns the daemon request/response contract only
  - keeps the privileged boundary reusable by the CLI, daemon server, and tool adapter
- `apps/sched-claw/crates/daemon-core`
  - owns daemon client and server transport plus privileged process lifecycle enforcement
  - keeps Unix-socket I/O and rollout safety checks out of the host composition crate
- `apps/sched-claw`
  - remains the host composition crate
  - owns bootstrap, CLI, REPL, display, doctor surfaces, skill materialization, and operator-facing composition

The intent is to keep the host focused on agent runtime composition and a narrow
privileged boundary. Collection, analysis, plotting, and code generation logic
belongs in skill scripts, not in more host-side workflow crates.

## Runtime shape

- `sched-claw` provides:
  - one-shot execution: `sched-claw exec "prompt"`
  - a simple line REPL: `sched-claw repl`
  - a product-facing readiness surface:
    - `sched-claw doctor --style table`
    - `sched-claw doctor --style plain`
  - skill-first helper scripts for collection and analysis:
    - `apps/sched-claw/skills/sched-perf-collection/scripts/collect_perf.sh`
      with `--driver host|daemon`
    - `apps/sched-claw/skills/sched-perf-analysis/scripts/bootstrap_uv_env.sh`
    - `apps/sched-claw/skills/sched-perf-analysis/scripts/analyze_perf_csv.py`
    - `apps/sched-claw/skills/sched-perf-analysis/scripts/compose_perf_evidence.py`
    - `apps/sched-claw/skills/sched-perf-analysis/scripts/render_perf_report.sh`
    - `apps/sched-claw/skills/sched-ext-codegen/scripts/scaffold_sched_ext_candidate.sh`
    - `apps/sched-claw/skills/sched-ext-codegen/scripts/scaffold_design_brief.sh`
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
    - `sched-claw daemon collect-perf --pid 4242 --duration-ms 1000 --output-dir artifacts/perf-a`
- `sched-claw-daemon` is a separate binary intended to run with elevated
  privileges. It manages one active sched-ext deployment at a time, captures the
  child process logs, and exposes:
  - `status`
  - `activate`
  - `collect_perf`
  - `stop`
  - `logs`

## Skills

Built-in skills are materialized under:

- `.nanoclaw/apps/sched-claw/builtin-skills`

They cover:

- product readiness and operator dependency checks
- workload contract definition and metric policy
- scheduler-specific performance collection SOPs and evidence persistence
- scheduler-specific performance analysis SOPs and durable fact or inference capture
- Linux scheduler-focused evidence collection and triage
- translating evidence and analysis into explicit sched-ext design records
- sched-ext build, verifier, run, and rollout safety loops
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

## Artifact-first workflow

The preferred path is:

- workload context captured as normal files under `.nanoclaw/` or another
  durable workspace directory
- collection via shell commands or helper scripts such as
  `skills/sched-perf-collection/scripts/collect_perf.sh`
- analysis via repo-local scripts such as
  `skills/sched-perf-analysis/scripts/analyze_perf_csv.py`,
  `skills/sched-perf-analysis/scripts/compose_perf_evidence.py`, and
  `skills/sched-perf-analysis/scripts/render_perf_report.sh`
- sched-ext code scaffolding via repo-local scripts such as
  `skills/sched-ext-codegen/scripts/scaffold_sched_ext_candidate.sh` and
  `skills/sched-ext-codegen/scripts/scaffold_design_brief.sh`
- privileged rollout only through `sched_ext_daemon`
- bounded privileged perf attachment also goes through `sched_ext_daemon`; do
  not replace it with `sudo perf ...` shell escapes

Reference sched-ext starting points still live under
`apps/sched-claw/templates/sched_ext/`, but they are reference material for
skills and scripts rather than a required host materialization path.

The external design references that shape this thin-host direction are tracked
in:

- `docs/sched-ext-industrial-and-autotuning-notes.md`

## Doctor

`sched-claw doctor` is the operator-facing readiness surface for the host. It
does not change the model-visible tool surface; it checks whether the current
workspace and machine are ready for real sched-claw use.

Current checks include:

- selected provider credentials for the active primary model
- builtin sched-claw skills, helper scripts, and shared `apps/code-agent/skills` availability
- privileged daemon socket reachability
- kernel release plus required `sched_ext` / BPF / cgroup config options
- core toolchain availability such as `clang`, `bpftool`, `perf`, `uv`, and `python3`
- uv-managed analysis helper compatibility against the repository requirements
- kernel prerequisites such as BTF and cgroup v2
- `perf_event_paranoid` visibility for non-root collection compatibility

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
  - self-bootstrapped sysbench runner image recipe:
    - `apps/sched-claw/scripts/docker/sysbench-runner.Dockerfile`
  - builtin skill:
    - `mysql-sysbench-tuning`

The wrapper scripts do two things:

- write a durable workload-context note next to the demo artifacts so the agent
  can read an explicit launcher contract, metric priorities, and rollback notes
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
- only privileged activation, stop, log inspection, and bounded attach-style
  perf capture use `sched_ext_daemon`

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

- REPL command parsing
- session history reference resolution and transcript rendering
- startup catalog alias resolution
- table/plain rendering for tool, skill, session, and daemon inspection views
- demo script dry-run bootstrapping for LLVM/clang and MySQL sysbench
- builtin skill helper script syntax and scaffolding checks

The root-required path still depends on a host that can actually run `sudo`
interactively or through a service manager. The repository now includes the
startup script for that path, but the repository test suite does not assume
passwordless root.
