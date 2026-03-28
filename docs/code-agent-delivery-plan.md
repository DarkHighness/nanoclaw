# Code Agent Delivery Plan

Date: 2026-03-28

Status: Active

## 1. Goal

This iteration changes the `apps/` delivery boundary and the example-app
architecture together:

- stop treating `apps/reference-tui` and `apps/code-agent` as two peer products
- converge the app layer on a single example product: `code-agent`
- split `code-agent` into a strict backend/frontend architecture
- keep TUI as the only shipped frontend for now
- make the backend reusable for future frontends such as Web
- use the existing substrate as the stable foundation, then close the product
  gap toward an industrial code agent surface

The implementation should follow repeated cycles:

1. design one bounded slice
2. implement conflict-free slices in parallel
3. validate and close the slice
4. use the new state as the input for the next cycle

## 2. Current State

## 2.1 App Boundary

Today the app workspace still exposes two host apps:

- `apps/reference-tui`
- `apps/code-agent`

The repository README also documents them as parallel products instead of one
product plus one archived reference shell.

## 2.2 Structural Reality

`code-agent` is not frontend/backend separated yet.

Current coupling points:

- `apps/code-agent/src/main.rs`
  - owns config loading, tracing, sandbox, tool registry, plugin boot, MCP
    wiring, runtime construction, and TUI startup in one binary crate
- `apps/code-agent/src/tui/mod.rs`
  - owns `AgentRuntime`, `RuntimeCommandQueue`, turn lifecycle, slash-command
    handling, approval bridge wiring, and rendering lifecycle together
- `apps/code-agent/src/tui/observer.rs`
  - writes runtime progress directly into TUI state instead of a frontend-neutral
    event/session model
- `apps/code-agent/src/tui/approval.rs`
  - approval flow is a TUI-local oneshot bridge instead of a backend capability

At the same time, `reference-tui` still duplicates large parts of host boot:

- provider backend construction
- plugin and MCP activation
- hook runner construction
- sandbox policy derivation
- startup summary and operator shell concerns

Relevant files:

- `apps/code-agent/src/main.rs`
- `apps/code-agent/src/options.rs`
- `apps/code-agent/src/provider.rs`
- `apps/code-agent/src/tui/*`
- `apps/reference-tui/src/boot.rs`
- `apps/reference-tui/src/config.rs`
- `apps/reference-tui/src/app.rs`
- `apps/reference-tui/src/boot/provider.rs`

## 3. Missing Capabilities

The gap splits into two kinds:

- substrate exists, but `code-agent` does not expose it as a product capability
- substrate or app boundary is still incomplete for industrial use

## 3.1 P0: Must Fix Before Calling It The Single Example App

### A. No strict backend/frontend boundary

Symptoms:

- TUI owns runtime execution instead of calling a backend API
- no frontend-neutral command, session, event, approval, or snapshot contract
- no reusable backend crate/service for future Web frontend

Code evidence:

- `apps/code-agent/src/main.rs`
- `apps/code-agent/src/tui/mod.rs`
- `apps/code-agent/src/tui/observer.rs`
- `apps/code-agent/src/tui/approval.rs`

### B. Two apps still exist with overlapping boot responsibilities

Symptoms:

- duplicated provider build logic
- duplicated plugin/MCP/hook startup merge flow
- duplicated host tracing and workspace boot

Code evidence:

- `apps/code-agent/src/provider.rs`
- `apps/reference-tui/src/boot/provider.rs`
- `apps/code-agent/src/main.rs`
- `apps/reference-tui/src/boot.rs`

### C. `code-agent` uses in-memory run storage only

This blocks industrial session continuity, replay, export, and frontend
reattachment.

Code evidence:

- `apps/code-agent/src/main.rs`
  - `let store = Arc::new(InMemoryRunStore::new());`

Contrast:

- `apps/reference-tui/src/boot.rs`
  - already uses `store` crate integration and run listing/export flows

### D. App delivery boundary is still unclear in docs and workspace

Symptoms:

- `apps/Cargo.toml` lists both products
- `README.md` documents both products as first-class apps

Code evidence:

- `apps/Cargo.toml`
- `README.md`

## 3.2 P1: High-Value Product Gaps

### A. No frontend-neutral session API

Missing pieces:

- session handle abstraction
- attach/detach/reconnect model
- command submission API
- runtime event stream API
- approval request/response API
- query API for transcript, tools, skills, MCP, runs, and token ledger

Current state:

- runtime events are pushed directly into TUI state
- slash commands mutate local UI state and local runtime handle

Code evidence:

- `apps/code-agent/src/tui/mod.rs`
- `apps/code-agent/src/tui/observer.rs`

### B. `code-agent` does not expose the operator surfaces already present in `reference-tui`

Missing or weaker product surfaces:

- run history listing and replay
- export run / transcript
- MCP server, prompt, and resource inspection
- richer startup diagnostics and operator summary

Code evidence:

- `apps/reference-tui/src/app.rs`
- `apps/reference-tui/src/app/commands/*`
- `apps/reference-tui/src/boot/summary.rs`
- `apps/code-agent/src/tui/mod.rs`

### C. Approval is frontend-specific

Missing pieces:

- backend-owned approval state
- reusable approval cache/policy model
- multi-frontend-safe approval contract

Code evidence:

- `apps/code-agent/src/tui/approval.rs`
- `apps/reference-tui/src/app/approval.rs`

### D. Managed LSP helper policy is intentionally looser than the foreground runtime

This is already called out in code comments and should be closed before claiming
industrial robustness.

Code evidence:

- `apps/code-agent/src/main.rs`
  - managed LSP currently uses permissive policies for background helper paths

## 3.3 P2: Alignment Gaps Toward Industrial Code Agents

### A. No multi-frontend host protocol

The product needs an explicit protocol even if it is in-process at first:

- command types
- event types
- approval request/response envelopes
- session snapshot schema

### B. No session persistence model for long-running product workflows

The runtime substrate supports replayable runs, but `code-agent` does not yet
ship a durable product boundary around them.

### C. No consolidated operator view for subagents, tasks, approvals, MCP, memory, and hooks

The runtime can do much of this internally, but the app product does not expose
a coherent operational surface yet.

## 4. Target Architecture

The shipped example app should become one product family with two internal
layers:

```text
apps/
└── code-agent
    ├── backend
    │   ├── app service / session manager
    │   ├── runtime boot and tool registry assembly
    │   ├── persistent store integration
    │   ├── plugin / MCP / memory / hook activation
    │   ├── command API
    │   ├── event stream API
    │   └── approval API
    └── frontend-tui
        ├── render
        ├── keymap / composer
        ├── inspector panes
        └── adapter over backend API
```

The separation rule is strict:

- frontend does not own `AgentRuntime`
- frontend does not build tool registries
- frontend does not manage approval channels directly
- frontend only talks to the backend through typed commands, snapshots, and
  event subscriptions

## 5. Delivery Boundary Decision

Target outcome for `apps/`:

- only `code-agent` is presented as the shipped example application
- `reference-tui` is treated as migration source material, then archived or
  removed from the active app workspace

This does not mean deleting functionality first. It means:

1. extract reusable capability into the `code-agent` backend/frontend split
2. validate parity for the required operator surfaces
3. remove `reference-tui` from active delivery, docs, and workspace membership

## 6. Iterative Execution Plan

## 6.1 Iteration 0: Design Baseline

Deliverables:

- this plan
- backend/frontend contract draft
- migration map from `reference-tui` into `code-agent`

Acceptance:

- architecture, scope, and ordering are explicit
- P0 and P1 gaps are prioritized

## 6.2 Iteration 1: Structural Split

Goal:

- extract a frontend-neutral `code-agent` backend layer

Planned slices:

- slice A: move runtime boot/config/provider/plugin/MCP assembly into backend
- slice B: introduce typed session commands, snapshots, and progress events
- slice C: move approval orchestration into backend-owned state
- slice D: convert TUI into a pure adapter over backend APIs

Parallelization note:

- A and B can proceed in parallel once the contract file exists
- C depends on B
- D depends on A and B, but rendering-only adjustments can start early

Acceptance:

- TUI no longer owns `AgentRuntime`
- `main.rs` becomes a thin launcher
- backend can be reused by a future Web frontend without pulling in ratatui

## 6.3 Iteration 2: Capability Absorption From `reference-tui`

Goal:

- make `code-agent` the single real example app, not just the nicer shell

Planned slices:

- slice A: switch `code-agent` to durable store integration
- slice B: add run history, replay, and export surfaces
- slice C: add MCP/resource/prompt inspection surfaces where appropriate
- slice D: port startup diagnostics and app boot summary

Parallelization note:

- A can proceed in parallel with D
- B depends on A
- C depends on backend query APIs from iteration 1, not on TUI rendering

Acceptance:

- `code-agent` covers the essential operator flows that only `reference-tui`
  currently exposes

## 6.4 Iteration 3: Industrial Hardening

Goal:

- reduce the remaining gap against industrial code-agent products

Planned slices:

- slice A: session reattach/reopen model
- slice B: approval caching and policy UX
- slice C: subagent/task operational visibility
- slice D: LSP helper sandbox/approval alignment
- slice E: contract tests for backend/frontend protocol and state recovery

Acceptance:

- backend behavior is testable without the TUI
- approval, session, and run flows survive frontend restarts
- product boundaries are explicit enough for future Web work

## 6.5 Iteration 4: Delivery Cleanup

Goal:

- make the repository tell one coherent app story

Planned slices:

- remove `reference-tui` from active workspace membership
- update README and docs to present only `code-agent`
- archive or remove obsolete app docs and examples

Acceptance:

- `apps/` no longer communicates two parallel example products
- active docs describe only the `code-agent` product family

## 7. Parallel Work Map

Once iteration 1 starts, the work should be split into conflict-free tracks:

- `backend_boot_worker`
  - config loading
  - provider construction
  - tool registry assembly
  - plugin/MCP boot
- `backend_protocol_worker`
  - command/event/snapshot types
  - session service traits and adapters
- `approval_worker`
  - backend approval queue/state
  - frontend approval adapter
- `tui_adapter_worker`
  - TUI state consumption from backend snapshots/events
  - removal of direct runtime ownership
- `migration_reviewer`
  - diff `reference-tui` feature set against migrated `code-agent`

Constraint:

- only one worker may own a specific file tree at a time
- shared contract files must land first

## 8. First Concrete Implementation Order

The first implementation pass should be:

1. define backend contract modules and move `code-agent` boot/runtime assembly
   behind them
2. make TUI consume the backend instead of `AgentRuntime`
3. switch run storage to durable store integration
4. pull over the highest-value `reference-tui` operator surfaces
5. remove `reference-tui` from the active delivery boundary

## 9. Acceptance Checklist

The app split is only accepted when all of the following are true:

- `code-agent` frontend compiles without importing runtime boot details
- backend compiles without TUI dependencies
- run persistence is durable
- approval flow is backend-owned
- at least one frontend-neutral session/event contract test exists
- active docs describe `code-agent` as the only shipped example app
- `reference-tui` is no longer part of the active `apps` delivery path
