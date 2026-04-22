# sched-claw

`sched-claw` is a thin host built on the same substrate as `code-agent`, but it
removes the heavy management and TUI surfaces and narrows the runtime around one
job: investigate Linux scheduling behavior, generate a new `sched-ext`
scheduler, and hand privileged rollout work to a dedicated daemon.

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
  - a local experiment substrate for workload contracts, baselines, candidates, and scoring:
    - `sched-claw experiment list --style table`
    - `sched-claw experiment init --id demo --workload-name bench --primary-metric latency_ms --primary-goal minimize`
    - `sched-claw experiment add-candidate demo --candidate-id locality-v1 --template dsq_local`
    - `sched-claw experiment record-baseline demo --label cfs-baseline --artifact-dir artifacts/baseline --metric latency_ms=12.4`
    - `sched-claw experiment record-candidate demo --candidate-id locality-v1 --label run-a --artifact-dir artifacts/cand-a --metric latency_ms=9.1`
    - `sched-claw experiment score demo --style table`
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

- Linux scheduler-focused evidence collection and triage
- Translating workload evidence into sched-ext policy and rollout steps

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
- `:experiments`
- `:experiment <id>`
- `:score <id>`
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

The intended loop is:

1. `experiment init`
   - define the workload contract, primary metric, and guardrails
2. `experiment add-candidate`
   - register the candidate policy template, source path, build command, daemon argv, and knobs
3. `experiment record-baseline`
   - store one or more CFS baseline runs with artifact paths and measured metrics
4. `experiment record-candidate`
   - store one or more sched-ext runs for a specific candidate
5. `experiment score`
   - compare candidate medians against the baseline and classify each candidate as `promote`, `revise`, `blocked`, or `incomplete`

This is intentionally a host-local substrate, not a new model-visible tool.
Agents are expected to call these commands through the existing shell tool so
the visible tool surface stays minimal.

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
- REPL command parsing
- session history reference resolution and transcript rendering
- startup catalog alias resolution
- table/plain rendering for tool, skill, session, and daemon inspection views

The root-required path still depends on a host that can actually run `sudo`
interactively or through a service manager. The repository now includes the
startup script for that path, but the repository test suite does not assume
passwordless root.
