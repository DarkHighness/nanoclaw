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
  - local inspection helpers such as `sched-claw tool list`
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

The model is expected to load those skills with `skill_view` before inventing
new measurement recipes.

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

The root-required path still depends on a host that can actually run `sudo`
interactively or through a service manager. The repository now includes the
startup script for that path, but the repository test suite does not assume
passwordless root.
