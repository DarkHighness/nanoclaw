# Code Agent Delivery Plan

Date: 2026-03-28

Status: Active

## Goal

The `apps/` workspace should converge on a single delivered example product:
`code-agent`.

`apps/reference-tui` remains in-tree only as temporary migration source
material. It is no longer an actively maintained product direction and should
not define the public delivery shape of the repository.

This plan drives two linked changes:

- split `code-agent` into a strict backend/frontend application boundary
- align `code-agent` toward an industrial code-agent surface informed by Codex,
  Claude Code, and OpenCode

Design companion:

- `docs/session-model-design.md`
  - canonical `Session` / `AgentSession` model that defines the pending
    repository-wide rename away from `Run*`

## Current Phase Status

Shipped in the first implementation slice:

- `apps/Cargo.toml` now defaults to `code-agent`
- `code-agent` now has explicit `backend/` and `frontend/tui/` module roots
- the TUI no longer holds `AgentRuntime` directly and instead talks to a
  backend-owned session facade

Shipped in the second implementation slice:

- `main.rs` now acts as a thin process entrypoint plus frontend composition
- backend session construction now lives in `backend/boot.rs`
- `code-agent` now prefers a file-backed run store with in-memory fallback
- durable run persistence is available to the backend even before the replay and
  export UI surfaces move over

Shipped in the third implementation slice:

- backend session startup now exposes a structured snapshot with provider,
  store, run-count, tool, skill, and sandbox metadata
- the TUI startup inspector now renders backend-owned boot facts instead of
  reconstructing host state from ad hoc session getters
- repository top-level docs now treat `reference-tui` as temporary migration
  source material instead of an actively maintained app

Shipped in the fourth implementation slice:

- `code-agent` backend now owns durable history lookup, replay, and export
  services
- the TUI now exposes persisted history browsing and export on top of
  backend-owned store access
- durable history replay/export no longer depends on the legacy shell staying
  product-shaped

Shipped in the fifth implementation slice:

- the TUI slash-command parser now uses `clap` instead of ad hoc string splits
- product-facing host terminology now converges on `session` even while the
  durable substrate store remains run-keyed
- the TUI session/history panes now render backend snapshots and history views
  with sectioned inspector output instead of flat text blobs

Shipped in the sixth implementation slice:

- backend boot is now decomposed across focused helpers for host inputs,
  sandbox/preamble construction, runtime tooling, and MCP/diagnostic summaries
- `code-agent` now owns MCP server/prompt/resource inspection instead of
  depending on the legacy shell for that operator workflow
- startup diagnostics are now backend-owned snapshots rendered by the TUI
  through dedicated `/diagnostics`, `/mcp`, `/prompts`, `/resources`,
  `/prompt`, and `/resource` commands

Shipped in the seventh implementation slice:

- backend approval prompts and approval decisions now live in a frontend-neutral
  coordinator instead of a TUI-owned bridge
- runtime progress now flows through a backend-owned event stream so prompt,
  steer, and compact operations can be rendered by any frontend without
  constructing runtime observers locally
- `main.rs` and the TUI no longer assemble runtime approval handlers or direct
  observer plumbing as part of frontend composition

Shipped in the eighth implementation slice:

- runtime compaction now rotates the root `AgentSessionId` instead of keeping a
  single long-lived runtime window past a history-boundary rewrite
- session-wide token accounting now aggregates across rotated root
  `AgentSession`s instead of losing pre-compaction usage
- `code-agent` now refreshes its active root-agent-session snapshot after
  prompt/steer and manual compaction operations

Shipped in the ninth implementation slice:

- `/new` and `/clear` now converge on the same backend-owned top-level session
  operation instead of keeping `/clear` as a frontend-only pane reset
- starting a fresh session now preserves prior sessions in durable history
  instead of treating clear/new as destructive deletion
- queued prompt/steer commands are now dropped when the operator starts a new
  top-level session so old work does not leak across session boundaries

Still pending in the next slices:

- frontend-neutral session-operation contracts beyond startup, approval, and
  event snapshots
- remaining docs and workspace cleanup before `reference-tui` can be retired

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
- product-facing `session`, `continue`, `resume`, and `fork` terminology rather
  than exposing raw persistence identifiers as the primary UX noun

## Current Repository Diagnosis

Primary code evidence:

- `apps/code-agent/src/main.rs`
- `apps/code-agent/src/frontend/tui/*`
- `apps/reference-tui/src/boot.rs`
- `apps/reference-tui/src/app.rs`
- `apps/Cargo.toml`
- `README.md`

Current problems:

- `code-agent` boot is now split across helper modules, but backend-owned host
  contracts are still incomplete for a future non-TUI frontend.
- the backend session now exposes startup, approval, and event contracts, but
  it still lacks a true runtime resume/reattach lifecycle above persisted
  history.
- explicit `AgentSession` lifecycle boundaries now exist for compaction, but
  full persisted runtime reattach still needs backend-owned contracts.
- legacy `reference-tui` code still duplicates host responsibilities that now
  belong in `code-agent`.
- `code-agent` now owns durable session browsing/replay/export plus MCP
  inspection, but true runtime resume/reattach still has not moved above the
  stored history catalog.
- product-facing commands now say `session`, but the host still lacks a true
  runtime resume/reattach path above the stored run catalog.
- repository docs and app workspace defaults were tightened, but the app
  workspace still contains transitional code that must be retired later.

## Gap Model

### P0

- strict backend/frontend split is incomplete
- frontend-neutral session-operation contract is incomplete
- `apps/` delivery boundary is still transitional while legacy code stays
  in-tree

### P1

- legacy host capabilities still need migration into `code-agent`
  - richer session/subagent operator surfaces
  - remaining legacy-only host controls
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
- `reference-tui` is temporary migration source material, not a second product
  direction

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

- migrate the remaining durable host capabilities into `code-agent`

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

Current note:

- approval and runtime event flow are now backend-owned contracts, but true
  runtime resume remains a pending backend capability rather than an
  implemented feature.

### Phase E: Delivery Cleanup

Goal:

- make `code-agent` the only actively delivered example app

Acceptance:

- `apps/Cargo.toml` default delivery path is `code-agent`
- README describes `reference-tui` as temporary migration source material only
- remaining `reference-tui` code is either archived, internal, or explicitly
  transitional on the path to deletion
