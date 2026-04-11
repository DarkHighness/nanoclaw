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
    and tool output formatting helpers.
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
- Test-only helpers should be behind `#[cfg(test)]` or deleted, never kept alive
  with blanket dead-code suppression.

## UI direction

The default theme now shifts to a more Codex-like neutral shell:

- darker graphite background
- softer stone text instead of bright blue-heavy contrast
- restrained teal accent for focus states
- muted gold/green user-assistant contrast that reads clearly without looking
  like a dashboard

This keeps the UI calm and tool-like rather than ornamental.

## Validation

Validated with:

- `cargo check --manifest-path apps/Cargo.toml -p code-agent`
- `cargo test --manifest-path apps/Cargo.toml -p code-agent-contracts -p code-agent-config -p code-agent-backend -p code-agent-tui -p code-agent`

## Remaining follow-up

The current split establishes real package boundaries and removes the duplicated
monolith from the binary crate. The next refinement step should be to narrow the
public backend facade further so the TUI depends on an even smaller explicit
session contract instead of a broad host API surface.
