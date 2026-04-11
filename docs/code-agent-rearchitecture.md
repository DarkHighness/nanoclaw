# Code Agent Rearchitecture

Date: 2026-04-11

Status: Active

## Why the previous design kept failing

The earlier `code-agent` iterations did create a directory-level split, but not
a real system boundary. The result was a host that looked layered while still
behaving like a monolith.

The recurring failure patterns were:

- **Facade without compilation boundaries**
  `apps/code-agent/src/backend/` and `apps/code-agent/src/frontend/` existed,
  but both still lived inside one package. Changes in the TUI, backend boot,
  theme handling, provider selection, and host config all recompiled and
  evolved together.
- **God files instead of cohesive components**
  `session.rs`, `frontend/tui/mod.rs`, and `frontend/tui/state.rs` accumulated
  orchestration, persistence, rendering state, runtime control, and operator
  workflow logic in the same units.
- **Product code mixed with substrate code**
  Host-only concerns such as theme catalogs, statusline policy, approval
  defaults, and TUI rendering helpers were effectively treated as substrate
  implementation details.
- **Pseudo-decoupling through module paths**
  The TUI consumed backend-owned types directly, so the frontend contract was
  whatever the backend happened to expose at that moment instead of a deliberate
  host boundary.
- **Dead code hidden by structure drift**
  Once responsibilities were no longer explicit, helper functions survived long
  after the call sites moved. The codebase then needed `allow(dead_code)`-style
  pressure relief instead of honest deletion or test-only isolation.

## New architecture

The host is now split into app-local packages under `apps/code-agent/` rather
than being mixed into the repository-wide substrate `crates/` tree.

```text
apps/code-agent
├── Cargo.toml              # thin binary package
├── src/main.rs             # process entrypoint only
└── crates
    ├── backend            # session/runtime boot, provider wiring, host policy
    ├── config             # app-local config loading and persistence
    ├── contracts          # theme, statusline, preview, tool rendering helpers
    └── tui                # terminal UI and rendering
```

### Package responsibilities

- `code-agent-contracts`
  - Owns presentation-safe shared models and pure helpers.
  - Holds theme catalogs, statusline configuration, preview collapse utilities,
    tool output formatting helpers, and frontend interaction contracts.
  - Contains no host boot orchestration or runtime mutation logic.
- `code-agent-config`
  - Owns app-local config loading from `.nanoclaw/apps/code-agent.toml`.
  - Persists mutable operator settings such as theme selection.
  - Bridges host policy config into typed values without owning runtime boot.
- `code-agent-backend`
  - Owns runtime/session construction, provider mutation, approval flow,
    permission flow, history/export, MCP inspection, and session state control.
  - Depends on substrate crates and app-local config/contracts packages.
  - Does not own TUI rendering or terminal lifecycle.
- `code-agent-tui`
  - Owns the terminal shell, event loop, render tree, command parsing, and
    operator interaction flow.
  - Depends on the backend facade plus app-local contracts/config, not on
    substrate boot internals directly.
- `apps/code-agent`
  - Reduced to the actual binary shell: env load, tracing init, runtime setup,
    and composition of backend + TUI.

## Boundary rules

These rules are the core of the new design and should be preserved.

- Host-only packages live under `apps/code-agent/`, not in repository-wide
  substrate `crates/`.
- Pure presentation/config helpers belong in `contracts` or `config`, not in
  `backend`.
- Runtime mutation, persistence, and provider logic belong in `backend`, not in
  `tui`.
- Terminal lifecycle, key handling, and visual composition belong in `tui`, not
  in `backend`.
- The TUI should talk to `code-agent-backend` through a dedicated frontend
  facade, not by importing the raw session object directly throughout the shell.
- Test-only helpers should be behind `#[cfg(test)]` or deleted, never kept alive
  with blanket dead-code suppression.

## Second-pass tightening

The first pass established package boundaries. The second pass tightened the
frontend boundary and reduced the size of the TUI controller.

- `code-agent-backend` now exposes `CodeAgentUiSession` as the TUI-facing
  backend adapter.
  - This keeps the raw runtime/session object as a backend concern and gives
    the shell one explicit surface to depend on.
- `code-agent-tui` no longer keeps all operator behavior inside one file.
  - Composer and attachment logic moved into `frontend/tui/composer.rs`.
  - History rollback flow moved into `frontend/tui/history_rollback.rs`.
  - Startup/session-shell synchronization moved into
    `frontend/tui/session_shell.rs`.
- The TUI shell now uses a persistent top header plus a restrained welcome
  screen that preserves the ASCII splash as the brand mark instead of treating
  the logo as disposable chrome.

This is a better industrial direction because boundary intent is now encoded in
the code layout instead of being left as convention.

## Third-pass decoupling

The next pass removed another source of false layering: frontend interaction
types still lived effectively inside `backend` because the session and the
runtime handlers translated data ad hoc.

- Approval, permission-request, user-input, pending-control, and skill-list
  payloads now live in `code-agent-contracts::interaction`.
- `code-agent-backend` owns a single internal translation seam
  (`frontend_contract.rs`) that maps runtime-owned types into those contracts.
  - This removes duplicated permission-profile mapping from multiple files.
  - It also keeps session/coordinator modules focused on runtime control rather
    than frontend shaping.
- `CodeAgentUiSession` now returns contract-safe skill summaries instead of
  leaking `agent::Skill`, and it no longer exposes unused permission runtime
  snapshots to the TUI.
- The UI boundary is now expressed as `UIQuery`, `UICommand`, and async
  command/result protocol types rather than a widening method table.
  - The TUI renders from query snapshots.
  - Operator actions dispatch explicit commands.
  - Background or I/O-bearing work crosses the boundary through async commands.

This matters because the TUI boundary is now a deliberate host protocol, not
whatever runtime structures happened to be convenient to forward.

## Fourth-pass protocol extraction

The next pass moved the operator-facing session protocol and DTOs out of
`code-agent-backend` and into `code-agent-contracts`.

- `code-agent-contracts::ui` is now the single source of truth for:
  - session and live-task snapshots
  - history/export DTOs
  - MCP inspection summaries
  - render-time event payloads
  - `UIQuery`, `UICommand`, `UIResult`, `UIAsyncCommand`, and `UIAsyncResult`
- `code-agent-backend` now focuses on executing that protocol through
  `CodeAgentUiSession` rather than owning the protocol type definitions.
- `code-agent-tui` now imports operator-facing DTOs from `contracts::ui`
  instead of treating backend re-exports as its main type surface.

This matters because the dependency direction is now explicit:

- `contracts` defines what the operator shell may observe or request
- `backend` implements those requests
- `tui` renders and dispatches against the contract

That is materially closer to a transport-safe command/query architecture than a
wide in-process facade.

## Fifth-pass session breakup

The backend session root is still large, but the next pass has started turning
it into a set of domain modules instead of one expanding host controller.

- History rollback moved into `backend/session/history.rs`.
- Live-task orchestration moved into `backend/session/live_tasks.rs`.
- Approval, permission, and user-input control handling already live in their
  own session submodules.

This reduces one of the previous failure modes: changing operator workflows,
runtime controls, and background-task logic in the same file.

## Sixth-pass TUI flow breakup

The main TUI controller was still acting as a god object even after the earlier
package and protocol work. The next pass split behavior by responsibility
instead of continuing to grow `frontend/tui/mod.rs`.

- Keyboard and picker control flow now lives in
  `frontend/tui/interaction_keys.rs`.
- Turn lifecycle, queue drain, theme application, and prompt materialization
  now live in `frontend/tui/runtime_flow.rs`.
- Slash-command dispatch and history-oriented command handling now live in
  `frontend/tui/slash_commands.rs`.
- `frontend/tui/mod.rs` is reduced to shell wiring, the event loop, shared
  helpers, and tests rather than owning every operator workflow directly.

This matters because command handling, transient runtime orchestration, and raw
input choreography now change in separate modules with smaller recompilation and
review surfaces.

## UI direction

The UI changes are not just palette swaps. The shell now shifts toward a more
Codex-like operator surface:

- darker graphite background
- softer stone text instead of bright blue-heavy contrast
- restrained teal accent for focus states
- muted gold/green user-assistant contrast that reads clearly without looking
  like a dashboard
- persistent session header for workspace/model/runtime state
- reduced visual noise on startup with a compact text-led welcome view
- preserved ASCII splash as the brand mark, with the surrounding shell chrome
  simplified instead of deleting the logo entirely
- shell-first information hierarchy: session header, main transcript, side rail,
  composer, statusline

This keeps the UI calm and tool-like rather than ornamental.

## Validation

Validated with:

- `cargo check --manifest-path apps/Cargo.toml -p code-agent`
- `cargo test --manifest-path apps/Cargo.toml -p code-agent-contracts -p code-agent-config -p code-agent-backend -p code-agent-tui -p code-agent`

## Remaining follow-up

The current split establishes real package boundaries, adds a TUI-facing backend
facade, and removes a meaningful chunk of orchestration from the primary TUI
controller. It is still not the final industrial end-state.

The next refinement steps should be:

- split `frontend/tui/state.rs` so transcript state, composer state, and picker
  state do not evolve inside one file
- continue splitting `backend/session.rs` so lifecycle, resume/catalog lookup,
  memory refresh, and side-question handling are not co-located
- consider moving the remaining history-load/task-load DTO formatting helpers
  fully behind `contracts::ui`-owned adapters so the TUI only depends on
  backend for execution surfaces
