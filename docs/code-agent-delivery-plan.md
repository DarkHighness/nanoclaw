# Code Agent Delivery Plan

Date: 2026-03-28

Status: Active

## Goal

The `apps/` workspace should converge on a single delivered example product:
`code-agent`.

The active tree now carries only `apps/code-agent`. Historical shell material
survives only in archived planning documents and no longer defines the
repository delivery boundary.

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
- repository top-level docs now center `code-agent` as the delivered app

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

Shipped in the tenth implementation slice:

- historical `AgentSession` reattach is now a real backend capability instead
  of a history-only status check
- compaction now persists resume checkpoints so compacted transcript windows can
  be reconstructed when reattaching historical agent sessions
- `code-agent` now exposes typed backend session operations for fresh-session
  rotation and agent-session reattach so frontends do not need to compose
  multiple backend calls around those flows

Shipped in the eleventh implementation slice:

- `code-agent` now exposes a backend-owned agent-session inspection contract in
  addition to top-level session replay
- the TUI can open a specific `AgentSession` directly instead of limiting the
  operator surface to listing and resume-only actions
- agent-session inspection now includes transcript slices, token usage, and
  spawned subagent summaries

Shipped in the twelfth implementation slice:

- `code-agent` now exposes persisted child-task listing and task inspection
  contracts on top of the stored event log
- the TUI can browse persisted tasks directly instead of requiring operators to
  infer child activity only from agent-session summaries
- task inspection now includes prompt/result/artifact data plus the child
  session transcript when one exists

Shipped in the thirteenth implementation slice:

- `code-agent` now exposes a backend-owned live child-task contract on top of
  the runtime subagent executor
- the TUI can inspect currently attached child agents and cancel them without
  holding direct runtime subagent plumbing
- the first live operator slice intentionally stops at list/cancel so the host
  contract stays small while send/wait semantics are still being designed

Shipped in the fourteenth implementation slice:

- `code-agent` now exposes backend-owned live task steer and wait contracts in
  addition to list/cancel
- the TUI can send parent steering to a selected live child and wait on a
  single child task without freezing the main event loop
- live wait now runs as a dedicated background operator task so runtime
  approvals, transcript deltas, and activity updates continue rendering while
  the operator waits for child completion

Shipped in the fifteenth implementation slice:

- the TUI layout now presents runtime state as a denser product surface instead
  of a mostly flat debug shell
- session metadata moved into a `session radar` card grid, and the shell gained
  a dedicated live status strip between the top bar and body
- the composer now exposes prompt vs command mode more clearly and highlights
  the live-task command path alongside the normal prompt flow

Shipped in the sixteenth implementation slice:

- `code-agent` now exposes a backend-owned live task spawn contract in addition
  to the existing list/send/wait/cancel controls
- the TUI can launch new root child tasks with `/spawn_task <role> <prompt>`
  without holding direct runtime subagent plumbing
- host-originated live task operations now anchor themselves to the active
  top-level `SessionId` / `AgentSessionId` so durable task history can recover
  operator-created child work later

Shipped in the seventeenth implementation slice:

- the TUI layout was simplified toward a more Codex-like shell with a
  single-line status bar instead of a stacked header plus status strip
- the session radar panel was removed so more vertical space goes to the
  transcript and inspector surfaces
- transcript rendering and the composer were both reduced to a more minimal
  presentation so the UI reads as a tool instead of a dashboard

Shipped in the eighteenth implementation slice:

- the right-hand TUI rail was narrowed further so the transcript regains even
  more horizontal space
- the default inspector shell was reduced to a more neutral `Info` surface
  instead of a louder operator panel label
- the activity panel now behaves more like a compact recent log than a large
  parallel dashboard pane

Shipped in the nineteenth implementation slice:

- the TUI top line was reduced further to the minimum useful context:
  workspace, model, session, and status
- `agent session`, `git`, and zero-queue details were removed from the top bar
  so the transcript remains the primary visual target
- queue depth now only surfaces in the top line when it is non-zero

Shipped in the twentieth implementation slice:

- read-heavy slash-command surfaces such as `/help`, catalogs, search results,
  and export summaries now render in the main pane instead of the side rail
- the side rail was demoted to brief `Info` plus recent `Log` content only,
  which fits its narrow width much better
- transcript-backed replay flows such as `/session`, `/task`, and
  `/agent_session` still keep the transcript in the main pane with their detail
  inspector on the right

Shipped in the twenty-first implementation slice:

- the TUI palette was pulled away from blue accents toward a more neutral dark
  shell so the host feels closer to a restrained terminal tool
- the right-hand rail was compressed again and now only carries very short info
  plus a small recent log window
- the side rail content was shortened to match its narrow width instead of
  relying on wrapping dense structured output

Shipped in the twenty-second implementation slice:

- the dedicated TUI side rail was removed completely, leaving a single main
  surface for transcript and command views
- runtime status moved out of stacked headers and side logs and toward a
  simpler Codex-like terminal flow
- transcript entries now use clearer turn headers, compact inline progress
  updates, and explicit turn dividers instead of a flat undifferentiated stream
- approval prompts now render inline in the bottom pane while the shell removes
  more pane-title chrome and box-heavy structure

Shipped in the twenty-third implementation slice:

- transcript entries no longer expose visible `user>` / `tool>` / `approval>`
  tags; the shell now uses Codex-style textual prefixes such as `›`, `•`, `✔`,
  and `✗`
- live runtime progress now stays inline in the transcript while the very
  bottom row is reduced to contextual footer data instead of duplicating status
- approval prompts were tightened further toward the Codex snapshot style:
  question-first layout, inline command previews, and no heavy boxed chrome

Shipped in the twenty-fourth implementation slice:

- tool-heavy transcript cells now collapse their secondary detail lines by
  default so the main timeline remains readable during long tool runs
- `/details` restores the full inline tool payload stream on demand without
  moving operators into a separate inspector or modal
- user-turn separators now keep a larger visual gap around the transcript
  divider so `assistant -> tool -> user` boundaries read more clearly

Shipped in the twenty-fifth implementation slice:

- the legacy shell was removed from the active `apps/` workspace
- repository delivery docs now describe `code-agent` as the sole shipped app
- active workspace cleanup no longer depends on keeping a legacy shell in-tree

Still pending in the next slices:

- frontend-neutral contracts for richer live subagent/session operator workflows
  beyond the current spawn/list/send/wait/cancel slice

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
- `apps/Cargo.toml`
- `README.md`

Current problems:

- `code-agent` boot is now split across helper modules, but backend-owned host
  contracts are still incomplete for a future non-TUI frontend.
- `code-agent` now owns durable session browsing/replay/export plus MCP
  inspection, and historical `AgentSession` reattach is now implemented for
  histories that carry compaction checkpoints.
- repository docs and app workspace now align on `code-agent` as the sole
  delivered app, but richer operator workflows still need further product work.

## Gap Model

### P0

- strict backend/frontend split is incomplete

### P1

- subagent execution is available, but live control and richer orchestration
  still are not exposed as a strong product experience

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
- `code-agent` is the only delivered host application in the active tree

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

- approval and runtime event flow are now backend-owned contracts, and
  historical `AgentSession` reattach is available. Older compacted histories
  without resume checkpoint metadata still fall back to history-only browsing.
- fresh-session reset and historical reattach now share a typed backend
  session-operation surface, but richer operator flows still need to move onto
  that same contract family.
- the transcript renderer now consumes a syntax-aware Markdown pipeline, and
  file mutation tools now emit structured diff previews so live and replay
  shells can show concrete edits instead of only completion summaries.

### Phase E: Delivery Cleanup

Goal:

- make `code-agent` the only actively delivered example app

Acceptance:

- `apps/Cargo.toml` contains only `code-agent`
- README describes `code-agent` as the sole delivered app
- retired shell material lives only in archive documents, not the active tree
