# Code Agent Tooling Expansion Plan

Date: 2026-04-12

Status: Active

## Progress Ledger

| Area | Status | Notes |
| --- | --- | --- |
| Phase 1: Task Model | Complete | `task_create`, `task_get`, `task_list`, `task_update`, `task_stop` are registered and backed by typed task records. |
| Phase 2: Monitor | Complete | `monitor_start`, `monitor_list`, `monitor_stop` are registered and surfaced through typed monitor/session flows. |
| Phase 3: Worktree Lifecycle | Not Started | No typed worktree lifecycle yet. |
| Phase 4: Checkpoint And Restore | Not Started | Rollback remains transcript/history-centric. |
| Phase 5: Diagnostics | Complete | `code_diagnostics` exists as a typed tool surface and no longer has a mirrored slash command. |
| Phase 6: Cron / Automation | Not Started | No scheduled execution tool family yet. |
| Phase 7: Code Search | Not Started | No semantic/index-backed `code_search` surface yet. |
| Phase 8: Browser / Computer Use | Not Started | No first-class browser session tools yet. |
| Phase 9: Notebook Editing | Not Started | Notebook work still falls back to generic file tooling. |
| Cross-cutting: Operator Slash Surface | In Progress | Tool-mirroring slash commands have been pruned; remaining slash commands are operator/session surfaces only. |
| Cross-cutting: Tool Review Surface | In Progress | Typed transcript cells exist, but operator review for running input/output/failure still needs a fuller design pass. |
| Cross-cutting: Skill Lifecycle & Self-Evolution | Not Started | Skills can be listed/resolved, but there is no verifier-backed skill creation, archival, or promotion loop yet. |

## Goal

Close the highest-value tool-surface gaps between `nanoclaw` and mature code
agents such as Codex, Claude Code, and OpenCode without re-fragmenting the
tool contract that was just normalized around typed availability, canonical
tool names, and `patch_files`.

This plan is intentionally execution-oriented. It does not restate the whole
alignment research note. It turns the current gap assessment into a staged
implementation sequence with concrete ownership boundaries, protocol changes,
validation expectations, and ship criteria.

Design companions:

- `docs/tool-protocol-alignment.md`
- `docs/code-agent-delivery-plan.md`
- `docs/session-model-design.md`

## Why A New Plan Exists

The current runtime already exposes a credible baseline:

- grounded file tools: `read`, `write`, `edit`, `patch_files`
- code search/navigation primitives: `glob`, `grep`, `list`,
  `code_symbol_search`, `code_document_symbols`, `code_nav`
- execution and browsing: `exec_command`, `write_stdin`, `web_fetch`,
  `web_search`
- host-mediated coordination: `update_plan`, `request_user_input`,
  `request_permissions`
- multi-agent handles: `spawn_agent`, `send_input`, `wait_agent`,
  `resume_agent`, `list_agents`, `close_agent`
- MCP resource bridging and dynamic tool registration

That surface is enough for local coding loops, but it still lacks several
workflow-level capabilities that industrial code agents treat as first-class:

- background observation instead of foreground-only command execution
- task objects instead of overloading plan state for execution tracking
- isolated worktree lifecycle management
- durable checkpoint / restore semantics for code state
- first-class diagnostics feedback instead of code-intel navigation alone
- scheduled/background automation
- richer modality and retrieval surfaces for browser, notebook, and semantic
  code search workflows

## External Baseline

This plan uses the following external baselines:

- Codex app features:
  - multi-agent threads, diff review, isolated worktrees, and automations
  - source: OpenAI, 2026-02-02, https://openai.com/index/introducing-the-codex-app/
- Claude Code tool reference:
  - `Monitor`, `CronCreate/CronDelete/CronList`, `EnterWorktree/ExitWorktree`,
    `NotebookEdit`, task tools, `TodoWrite`, and automatic LSP diagnostics
  - source: Anthropic, 2026, https://code.claude.com/docs/en/tools-reference
- Claude Code checkpointing:
  - durable checkpoints, rewind, restore code, restore conversation, summarize
  - source: Anthropic, 2026, https://code.claude.com/docs/en/checkpointing
- OpenCode tools and permissions:
  - `codesearch`, `task`, `skill`, `question`, `multiedit` permissions
  - source: OpenCode, 2026, https://opencode.ai/docs/tools/
  - source: OpenCode, 2026, https://opencode.ai/docs/permissions/
- OpenCode LSP:
  - diagnostics feed back into the agent loop
  - source: OpenCode, 2026, https://opencode.ai/docs/lsp/
- Hermes Agent skills baseline:
  - automated skill creation, agentskills.io-compatible `SKILL.md`, and an explicit skill-vs-tool boundary
  - source: Hermes Agent, 2026, https://hermes-agent.org/
  - source: Hermes Agent, 2026, https://hermes-agent.nousresearch.com/docs/developer-guide/adding-tools/

## Current Source Of Truth

The current runtime tool surface is defined in:

- [boot_runtime.rs](/home/twiliness/nanoclaw/apps/code-agent/crates/backend/src/backend/boot_runtime.rs)

The current tool protocol and registry are defined in:

- [tool.rs](/home/twiliness/nanoclaw/crates/types/src/tool.rs)
- [registry.rs](/home/twiliness/nanoclaw/crates/tools/src/registry.rs)

The current TUI operator surfaces relevant to this plan are defined in:

- [observer.rs](/home/twiliness/nanoclaw/apps/code-agent/crates/tui/src/frontend/tui/observer.rs)
- [history_rollback.rs](/home/twiliness/nanoclaw/apps/code-agent/crates/tui/src/frontend/tui/history_rollback.rs)
- [tool_render.rs](/home/twiliness/nanoclaw/apps/code-agent/crates/contracts/src/tool_render.rs)

## Remaining Gap Summary

### P0 Gaps

- Worktree lifecycle tools
- Checkpoint / restore tool family
- Skill lifecycle and self-evolution
- Tool review surface parity for running input/output/failure rendering

### P1 Gaps

- Cron / automation tools
- Code search beyond symbol search and text grep
- Skill archive / promotion workflow

### P2 Gaps

- Browser / computer-use tools
- Notebook editing

## Execution Principles

1. Keep one canonical tool per capability family.
   Do not reintroduce alias-heavy compatibility surfaces such as `patch` /
   `apply_patch`.

2. Prefer typed entities over string protocols.
   Task ids, monitor ids, worktree ids, checkpoint ids, cron ids, and their
   states must be enums / structs in `crates/types`, not inferred from
   transcript text.

3. Keep host-owned coordination distinct from model-owned content editing.
   `update_plan` remains high-level coordination state. Execution objects such
   as tasks, monitors, checkpoints, and worktrees get their own tool families.

4. Ship each phase end-to-end.
   Each slice must include protocol, runtime wiring, persistence, TUI surface,
   and tests before moving on.

5. Do not build UI-only features without substrate support.
   If the operator can inspect, resume, stop, or restore something, the
   capability must exist as a typed backend/runtime contract first.

6. Keep model tools and operator commands as separate surfaces.
   Model-visible tools do not automatically get slash commands. Slash commands
   exist for session control, review, history, export, attachments, and
   runtime supervision. Explicit skill invocation belongs to composer-native
   `$skill_name` directives, not to tool-mirroring slash commands.

7. Keep `update_plan` narrow.
   `update_plan` is host-owned coordination state, not a replacement for typed
   task, monitor, checkpoint, or worktree entities. If an object needs its own
   lifecycle, persistence, or operator controls, it should not be shoved into
   the plan.

## Phase 1: Task Model

### Objective

Introduce a first-class task object model so execution tracking stops leaking
into `update_plan`, live-task side channels, and transcript-only summaries.

### User-facing outcome

- The model can create, inspect, list, update, and stop tasks.
- Tasks can represent:
  - parent-authored work items
  - child-agent owned work
  - operator-created TODOs not tied to a live child

### New canonical tools

- `task_create`
- `task_get`
- `task_list`
- `task_update`
- `task_stop`

### Protocol work

- Add typed task ids and task states in `crates/types`.
- Define one canonical task status enum. Do not overload plan statuses.
- Add typed task dependencies instead of freeform dependency strings.
- Add typed task origin metadata:
  - user-created
  - agent-created
  - child-agent-backed
  - automation-backed

### Write set

- `crates/types/src/`
- `crates/tools/src/agentic/`
- `crates/runtime/src/`
- `apps/code-agent/crates/backend/src/backend/session*.rs`
- `apps/code-agent/crates/contracts/src/`
- `apps/code-agent/crates/tui/src/frontend/tui/`

### Required refactors

- Keep `update_plan` focused on plan/focus only.
- Move live-task operator summaries to typed task records.
- Stop treating child-agent handles as the only execution object.

### Acceptance

- A task can exist without a child agent.
- A child agent can back a task without collapsing into transcript-only state.
- Task history survives session reload.
- `/tasks` and `/task` render typed task records, not reconstructed prose.

### Exit criteria

- The model can manage tasks without touching `update_plan`.
- The TUI task views stop depending on transcript parsing for task state.

## Phase 2: Monitor

### Objective

Add a background observation tool comparable to Claude Code `Monitor`, so the
agent can keep watching logs, commands, directories, or polled status without
blocking the main conversation.

### User-facing outcome

- The model can start a monitor and continue other work.
- Each monitor feeds typed events back into the runtime.
- The operator can inspect active monitors and stop them.

### New canonical tools

- `monitor_start`
- `monitor_list`
- `monitor_stop`

### Protocol work

- Add `MonitorId`, `MonitorStatus`, and `MonitorEvent`.
- Reuse `exec_command` approval policy semantics for monitor launch, but do not
  reuse raw command-session ids as monitor ids.
- Define one structured event envelope for:
  - stdout line
  - stderr line
  - state change
  - completion
  - failure

### Write set

- `crates/types/src/`
- `crates/tools/src/process/`
- `crates/runtime/src/runtime/`
- `apps/code-agent/crates/backend/src/backend/session/`
- `apps/code-agent/crates/tui/src/frontend/tui/`

### Required refactors

- Extend runtime event flow so monitors can inject asynchronous events while a
  turn is otherwise idle.
- Keep monitor lifecycle separate from `write_stdin` session lifecycle.

### Acceptance

- A monitor can tail a file or long-running command and emit incremental lines.
- The transcript shows monitor events as typed monitor cells, not fake tool
  completions.
- Stopping a monitor is idempotent and survives stale stop requests.

### Exit criteria

- Background observation exists as a first-class tool family.
- There is no need to abuse `exec_command` plus manual polling for watch flows.

## Phase 3: Worktree Lifecycle

### Objective

Make isolated worktrees first-class runtime objects instead of an implicit
sandbox/workspace-root detail.

### User-facing outcome

- The model can create an isolated worktree, switch into it, and exit it.
- Subagents can opt into dedicated worktrees.
- The operator can inspect which worktree a task or child agent is using.

### New canonical tools

- `worktree_enter`
- `worktree_exit`
- `worktree_list`

### Protocol work

- Add typed worktree handles and lifecycle states.
- Define whether worktrees are:
  - session-scoped
  - task-scoped
  - child-agent-scoped
- Capture the relationship between worktree root and sandbox scope.

### Write set

- `crates/types/src/`
- `crates/runtime/src/subagent_impl.rs`
- `crates/sandbox/src/`
- `apps/code-agent/crates/backend/src/backend/`
- `apps/code-agent/crates/tui/src/frontend/tui/`

### Required refactors

- Stop treating worktree root as only a sandbox input.
- Teach task/child-agent summaries to surface worktree attachment explicitly.

### Acceptance

- A worktree can be entered and exited without corrupting the parent session.
- A child agent can be launched in an isolated worktree with clear ownership.
- Approval and transcript rendering surface worktree context explicitly.

### Exit criteria

- Worktree lifecycle becomes a typed, operator-visible tool family.

## Phase 4: Checkpoint And Restore

### Objective

Add durable code-state checkpoints and restore flows so rollback is not only a
transcript/history operation.

### User-facing outcome

- The system can restore:
  - code only
  - conversation only
  - both
  - summarize from a checkpoint boundary
- File mutation tools create checkpoint records automatically.

### New canonical tools

- `checkpoint_list`
- `checkpoint_restore`
- `checkpoint_summarize`

### Protocol work

- Add `CheckpointId`, checkpoint scope, and restore mode enums.
- Track file edits made by direct file tools separately from transcript-only
  history.
- Persist the mapping between visible turn boundaries and code checkpoints.

### Write set

- `crates/types/src/`
- `crates/runtime/src/session.rs`
- `crates/runtime/src/runtime/history.rs`
- `apps/code-agent/crates/backend/src/backend/session_history.rs`
- `apps/code-agent/crates/tui/src/frontend/tui/history_rollback*.rs`

### Required refactors

- Split current history rollback into:
  - transcript rewind
  - checkpoint restore
- Do not present code restore as a side effect of transcript rollback unless a
  real checkpoint exists.

### Acceptance

- File changes made through `write`, `edit`, and `patch_files` can be restored.
- Compaction and rotated agent sessions preserve checkpoint visibility.
- The TUI rollback surface clearly distinguishes transcript-only rewind from
  code-state restore.

### Exit criteria

- The system can safely recover code state without relying on git or operator
  manual cleanup.

## Phase 5: Diagnostics

### Objective

Expose diagnostics as a first-class tool/result surface and automatically feed
post-edit diagnostics back into the loop.

### User-facing outcome

- After file edits, the runtime can surface type errors and warnings without
  requiring the model to remember to run a build or a manual code-nav query.
- The operator can review diagnostics through normal transcript and review
  surfaces without a dedicated tool-mirroring slash command.

### New canonical tools

- `code_diagnostics`

### Protocol work

- Add typed diagnostic entries in `crates/types`.
- Define source attribution:
  - LSP
  - lexical fallback
  - build output if later integrated
- Keep diagnostics distinct from startup/system warnings.

### Write set

- `crates/types/src/`
- `crates/tools/src/code_intel/`
- `apps/code-agent/crates/backend/src/backend/`
- `apps/code-agent/crates/tui/src/frontend/tui/`

### Required refactors

- Reuse existing managed LSP `publishDiagnostics` plumbing instead of inventing
  a second diagnostics channel.
- Add a post-mutation trigger so diagnostics can be emitted after successful
  `write`, `edit`, or `patch_files`.

### Acceptance

- `code_diagnostics` returns typed warnings/errors for a path or workspace.
- Post-edit diagnostics can appear as typed follow-up cells or explicit
  structured tool results.
- Diagnostics rendering does not reuse startup diagnostics UI components or a
  bespoke `/code_diagnostics` operator command.

### Exit criteria

- LSP diagnostics become part of the normal agent loop instead of an internal
  cache only.

## Phase 6: Cron / Automation

### Objective

Add scheduled execution so the agent can perform repeated or deferred work
without an always-open interactive session.

### User-facing outcome

- The model can create one-shot or recurring scheduled tasks.
- Scheduled runs land back in a review queue or persisted task history.

### New canonical tools

- `cron_create`
- `cron_list`
- `cron_delete`

### Protocol work

- Add typed schedule expressions and next-run metadata.
- Separate session-scoped schedules from future persistent automations.

### Write set

- `crates/types/src/`
- `crates/runtime/src/`
- `apps/code-agent/crates/backend/src/backend/`
- `apps/code-agent/crates/tui/src/frontend/tui/`

### Acceptance

- Scheduled tasks can be listed, cancelled, and resumed after process restart
  if persistence is enabled.
- A completed scheduled run is inspectable as a task/result artifact.

### Exit criteria

- Repetitive background work no longer requires an always-on operator session.

## Phase 7: Code Search

### Objective

Fill the gap between `grep`/`glob` and `code_nav` with a true semantic or
index-backed code search surface.

### User-facing outcome

- The model can query code concepts that are not well served by plain grep or
  symbol lookup.

### New canonical tools

- `code_search`

### Protocol work

- Add query/result schema that can express:
  - semantic match score
  - matched symbol / snippet
  - path and span
  - backend used

### Write set

- `crates/types/src/`
- `crates/tools/src/code_intel/`
- `apps/code-agent/crates/backend/src/backend/boot_runtime.rs`

### Acceptance

- `code_search` returns ranked results that are not reducible to plain `grep`.
- The tool can fall back gracefully when no semantic backend is available.

### Exit criteria

- The agent no longer has to choose only between text grep and exact symbol
  navigation.

## Phase 8: Browser / Computer Use

### Objective

Add browser-native interaction for JS-heavy apps, authenticated flows, and
visual validation that web search/fetch cannot cover.

### User-facing outcome

- The model can open pages, inspect DOM/accessibility trees, click, type,
  submit, and capture screenshots.

### New canonical tools

- `browser_open`
- `browser_snapshot`
- `browser_click`
- `browser_type`
- `browser_eval`
- `browser_screenshot`
- `browser_close`

### Constraints

- Keep this behind an explicit feature gate and provider/model allowlist.
- Do not conflate browser automation with `web_fetch`.

### Acceptance

- The browser session lifecycle is typed and persisted separately from shell
  sessions.
- Approval policy for browser automation is explicit and not borrowed from
  generic web fetch/search.

### Exit criteria

- The agent can validate real running applications without requiring an
  external MCP server for every browsing task.

## Phase 9: Notebook Editing

### Objective

Support notebook-native workflows without degrading them into JSON patching.

### User-facing outcome

- The model can edit notebook cells, inspect execution order, and manipulate
  markdown/code cells as notebook objects.

### New canonical tools

- `notebook_read`
- `notebook_edit`

### Constraints

- Keep the notebook model typed around cells and metadata.
- Do not reuse `patch_files` against raw `.ipynb` JSON for interactive
  notebook work.

### Acceptance

- A notebook cell can be updated without destabilizing unrelated notebook
  metadata.
- The transcript renders notebook edits as notebook objects, not giant JSON
  diffs.

### Exit criteria

- Common notebook workflows no longer require brittle raw JSON edits.

## Cross-Cutting Workstream: Skill Lifecycle And Self-Evolution

### Objective

Bring skills closer to the stronger lifecycle used by Hermes-style agent
systems and the broader meta-agent roadmap: explicit discovery, creation,
evaluation, archival, and promotion instead of static skill loading only.

### User-facing outcome

- The runtime can distinguish:
  - loaded skills
  - candidate skills
  - archived skills
  - promoted skills
- Skill improvements can be proposed, reviewed, verified, and promoted without
  silently mutating the active skill set.
- Skill invocation stays explicit through `$skill_name`, while operator review
  can inspect the current loaded catalog and promotion history.

### Planned work

- extend skill metadata with lifecycle state and provenance
- define a verifier-backed path for reusable skill extraction from successful
  runs
- add archive/promotion storage instead of treating the workspace skill roots
  as the only source of truth
- align tool-vs-skill guidance with the Hermes split:
  - use skills for instruction-plus-existing-tool workflows
  - use tools for capabilities that require typed runtime integration

### Write set

- `crates/skills/`
- `crates/tools/src/agentic/skill.rs`
- `apps/code-agent/crates/backend/src/backend/`
- `apps/code-agent/crates/tui/src/frontend/tui/`
- `docs/meta-agent-evolution-plan.md`

### Exit criteria

- Skill state is no longer just "present on disk or not".
- Reusable skill evolution is auditable, reversible, and verifier-gated.

## Cross-Cutting Workstream: Operator Tool Review Surface

### Objective

Keep tool execution readable and operator-friendly while multiple tools are
running concurrently.

### User-facing outcome

- Running tool cells show typed status, intent, and in-flight output without
  dumping raw JSON.
- Completed tool cells clearly distinguish success, failure, and partial
  results.
- Operators can inspect input/output/review payloads without reopening raw
  protocol dumps.

### Planned work

- tighten typed tool render contracts for running vs terminal states
- improve structured input/output summaries and head/tail collapsing rules
- keep approval, running tool state, and final result distinct in the TUI
- avoid protocol text like `path>` / `query>` leaking into operator-facing
  rendering when structured fields exist

### Exit criteria

- The transcript remains readable under concurrent tool activity.
- The operator can tell what ran, what is still running, and what failed
  without expanding raw payloads.

## Recommended Delivery Order

### Wave A

- Phase 1: Task Model
- Phase 2: Monitor
- Phase 3: Worktree Lifecycle

Reason:

- These three phases establish the execution object model.
- They unblock cleaner multi-agent coordination and operator review.

### Wave B

- Phase 4: Checkpoint And Restore
- Phase 5: Diagnostics

Reason:

- These two phases establish safety and rapid repair loops.
- They directly improve trust in larger autonomous changes.

### Wave C

- Phase 6: Cron / Automation
- Phase 7: Code Search

Reason:

- These expand unattended and large-repo workflows after the core execution
  model is stable.

### Wave D

- Phase 8: Browser / Computer Use
- Phase 9: Notebook Editing

Reason:

- These are valuable, but they widen scope and platform complexity more than
  the earlier phases.

## Validation Rules

Every phase must ship with:

- typed protocol definitions in `crates/types`
- tool contracts in `crates/tools` or explicit host/runtime-native rationale if
  a capability does not belong in normal tool space
- runtime and backend integration tests
- TUI rendering and interaction tests
- persistence/reload tests if the phase introduces durable entities
- one docs update that records the new canonical tool names and removes any
  temporary compatibility language

## Explicit Non-Goals

- Do not reintroduce legacy alias trees to ease model transition.
- Do not add UI-only slash commands as substitutes for missing typed tools.
- Do not mirror model tools into slash commands unless the operator needs a
  distinct host control surface that the model tool cannot provide.
- Do not force skills through slash commands when `$skill_name` in the
  composer is the correct explicit invocation path.
- Do not encode new state machines as transcript strings.
- Do not let `update_plan` absorb task, checkpoint, or monitor semantics.
- Do not reintroduce slash commands that merely proxy model tools when a typed
  picker action or direct model tool invocation already exists.

## Immediate Next Step

The next highest-value implementation slices are:

1. Phase 3: add typed worktree lifecycle
2. Phase 4: split transcript rewind from durable checkpoint restore
3. cross-cutting: finish the operator tool review surface for concurrent
   running/completed tool cells
4. cross-cutting: add skill lifecycle state and promotion flow

Do not remove `update_plan` unless task, checkpoint, worktree, and operator
review flows can already express the same high-level coordination need without
regressing shared visibility.
