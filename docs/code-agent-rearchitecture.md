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
- The TUI shell now keeps the transcript flush with the top of the viewport and
  uses a restrained welcome screen that preserves the ASCII splash as the brand
  mark instead of treating the logo as disposable chrome.

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

## Sixth-pass backend memory breakup

The next backend hotspot was session memory management. Structured note refresh,
episodic capture, compaction handoff persistence, and side-question execution
were still embedded directly in `backend/session.rs`, even though they form one
host-owned policy area.

- Session-memory refresh scheduling and persistence now live in
  `backend/session/memory.rs`.
- Episodic daily-log capture now lives beside the structured session-note
  updater instead of being interleaved with general session control methods.
- Side-question context snapshots and side-question execution now live in the
  same module as the session-memory pipeline they depend on.
- `backend/session.rs` now keeps session construction, catalog/export
  operations, and other cross-domain control methods, while memory maintenance
  is triggered through the dedicated submodule.

This matters because the session root is no longer responsible for both
foreground operator commands and the background memory-maintenance pipeline.

## Seventh-pass TUI flow breakup

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

## Eighth-pass TUI state breakup

The remaining TUI hot spot was `frontend/tui/state.rs`. It still mixed
transcript DTOs, picker overlays, composer attachment protocol, history recall,
external-editor projection, and viewport helpers in one module.

- Transcript and inspector DTOs now live in `frontend/tui/state/transcript.rs`.
- Picker, pending-control, and history-rollback overlays now live in
  `frontend/tui/state/picker.rs`.
- Composer draft modeling, attachment normalization, submission snapshots, and
  editor/history behavior now live in `frontend/tui/state/composer.rs`.
- `frontend/tui/state.rs` now keeps only the shared root state, toast and
  viewport behavior, shared UI state wrapper, and git snapshot helpers.

This matters because the TUI state model now reflects three real subdomains:
transcript presentation, picker overlays, and composer workflow. The root state
module is still present as an integration seam, but it no longer owns every UI
behavior itself.

## Ninth-pass backend catalog breakup

The next backend hotspot was the operator-facing session catalog surface.
Listing sessions, searching by title, resolving human-entered references,
loading stored sessions, exporting transcripts, and counting persisted sessions
were still embedded directly in `backend/session.rs`, even though they form one
catalog-oriented boundary.

- Session and agent-session catalog listing now lives in
  `backend/session/catalog.rs`.
- Session-title note loading stays host-owned inside that module because the
  title comes from derived memory files rather than store-native transcript
  metadata.
- Operator reference resolution for session ids, prefixes, and title matches
  now lives beside the catalog queries it depends on.
- Task/session load and export helpers now live in the same module as the
  catalog lookup path they reuse.
- `backend/session.rs` keeps runtime lifecycle and resume orchestration, while
  persisted-catalog behavior moves behind the dedicated submodule.

This matters because the session root no longer mixes foreground runtime resume
control with operator catalog lookup and export plumbing.

## UI direction

The UI changes are not just palette swaps. The shell now shifts toward a more
Codex-like operator surface:

- darker graphite background
- softer stone text instead of bright blue-heavy contrast
- restrained teal accent for focus states
- muted gold/green user-assistant contrast that reads clearly without looking
  like a dashboard
- no persistent top status strip; runtime context stays in the footer and side rail
- reduced visual noise on startup with a compact text-led welcome view
- preserved ASCII splash as the brand mark, with the surrounding shell chrome
  simplified instead of deleting the logo entirely
- shell-first information hierarchy: main transcript, side rail, composer,
  footer context

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

- continue splitting `backend/session.rs` so lifecycle, runtime reattachment,
  startup snapshots, and remaining session-operation helpers are not
  co-located
- consider moving the remaining history-load/task-load DTO formatting helpers
  fully behind `contracts::ui`-owned adapters so the TUI only depends on
  backend for execution surfaces
