# Code Agent Delivery Plan

Date: 2026-03-28

Status: Active

## Goal

The `apps/` workspace should converge on a single delivered example product:
`code-agent`.

`apps/reference-tui` remains a migration source and validation shell during the
transition, but it should no longer define the public delivery shape of the
repository.

This plan drives two linked changes:

- split `code-agent` into a strict backend/frontend application boundary
- align `code-agent` toward an industrial code-agent surface informed by Codex,
  Claude Code, and OpenCode

## Current Phase Status

Shipped in the first implementation slice:

- `apps/Cargo.toml` now defaults to `code-agent`
- `code-agent` now has explicit `backend/` and `frontend/tui/` module roots
- the TUI no longer holds `AgentRuntime` directly and instead talks to a
  backend-owned session facade

Still pending in the next slices:

- host boot extraction out of `main.rs`
- durable run store, replay, and export migration
- frontend-neutral approval, event, and snapshot contracts

## External Product Signals

Official references used for the target shape:

- Codex subagents: <https://developers.openai.com/codex/subagents/>
- Codex subagent concepts: <https://developers.openai.com/codex/concepts/subagents/>
- Codex sandboxing: <https://developers.openai.com/codex/concepts/sandboxing/>
- Codex approvals and security: <https://developers.openai.com/codex/sandbox>
- Claude Code subagents: <https://docs.anthropic.com/en/docs/claude-code/subagents>
- Claude Code hooks: <https://docs.anthropic.com/en/docs/claude-code/hooks>
- OpenCode agents: <https://opencode.ai/docs/agents>
- OpenCode server: <https://opencode.ai/docs/server>
- OpenCode web: <https://opencode.ai/docs/web/>

The consistent product pattern across those systems is:

- backend-owned runtime/session state
- frontends as interchangeable clients
- agent-specific model / permission / tool contracts
- explicit subagent orchestration instead of prompt-only delegation
- durable session history, replay, and operator-visible controls

## Current Repository Diagnosis

Primary code evidence:

- `apps/code-agent/src/main.rs`
- `apps/code-agent/src/frontend/tui/*`
- `apps/reference-tui/src/boot.rs`
- `apps/reference-tui/src/app.rs`
- `apps/Cargo.toml`
- `README.md`

Current problems:

- `code-agent` is still a monolithic host. Boot, runtime construction, plugin
  activation, MCP wiring, subagent setup, and TUI composition all live in
  `main.rs`.
- the new session facade is only the first boundary cut; a future Web frontend
  still lacks typed host events, approval messages, and snapshots
- `reference-tui` and `code-agent` still duplicate host responsibilities and
  blur the delivery boundary.
- `code-agent` uses `InMemoryRunStore`, which is not sufficient for durable
  session history, replay, export, or audit.
- repository docs and app workspace defaults still imply two equally delivered
  example applications.

## Gap Model

### P0

- strict backend/frontend split is incomplete
- frontend-neutral command/event/snapshot/approval contract is missing
- durable run/session storage is missing from `code-agent`
- `apps/` delivery boundary is still ambiguous

### P1

- `reference-tui` capabilities still need migration into `code-agent`
  - runs/history/export
  - MCP prompt/resource inspection
  - startup diagnostics and host summaries
- approval flow is still too frontend-shaped
- subagent execution is available but not yet exposed as a strong product
  experience

### P2

- prompt/agent hook executors are still intentionally fail-closed stubs in the
  substrate
- managed LSP helpers still need tighter policy alignment with the foreground
  runtime path

## Target Architecture

```text
apps/code-agent
├── backend
│   ├── boot
│   ├── session
│   ├── store
│   ├── approval
│   └── host api
└── frontend
    ├── tui
    └── web (future)
```

Boundary rules:

- backend owns runtime, store, tool registry, plugin/MCP activation, approval
  coordination, and session lifecycle
- frontend owns rendering, input, local view state, and frontend-specific
  interaction patterns
- frontend talks to backend through typed commands, events, snapshots, and
  approval responses
- `reference-tui` is a migration source, not a second product direction

## Iteration Loop

Each phase follows the same loop:

1. design the next narrow slice
2. implement parallel, non-conflicting changes
3. validate the slice
4. update the live plan and move to the next slice

## Phases

### Phase A: Session Boundary

Goal:

- remove direct `AgentRuntime` ownership from the TUI
- introduce a backend-owned session facade

Acceptance:

- TUI depends on a backend session contract instead of holding runtime state
- no behavior regression in basic prompt / steer / compact flows

### Phase B: Backend Boot Extraction

Goal:

- move host boot logic out of `main.rs`
- isolate runtime/tool/plugin/MCP construction behind backend modules

Acceptance:

- `main.rs` becomes a thin composition entrypoint
- host boot tests move with backend modules

### Phase C: Durable Product Features

Goal:

- migrate `reference-tui` durable capabilities into `code-agent`

Acceptance:

- persistent run store
- replay/export/history surface
- startup diagnostics and MCP catalog visibility

### Phase D: Industrial Controls

Goal:

- strengthen approval, subagent observability, and session operations

Acceptance:

- approval is backend-owned state
- subagent lifecycle is visible and navigable
- session resume / reattach path is defined

### Phase E: Delivery Cleanup

Goal:

- make `code-agent` the only actively delivered example app

Acceptance:

- `apps/Cargo.toml` default delivery path is `code-agent`
- README describes `reference-tui` as migration/reference only
- remaining `reference-tui` code is either archived, internal, or explicitly
  transitional
