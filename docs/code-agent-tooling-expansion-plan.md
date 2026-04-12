# Code Agent Tooling Expansion Plan

Date: 2026-04-12

Status: Active

## Progress Ledger

| Area | Status | Notes |
| --- | --- | --- |
| Phase 1: Task Model | Complete | `task_create`, `task_get`, `task_list`, `task_update`, `task_stop` are registered, `update_plan` has been removed, and the TUI now restores live/current task state from typed task events. |
| Phase 2: Monitor | Complete | `monitor_start`, `monitor_list`, `monitor_stop` are registered and surfaced through typed monitor/session flows. |
| Phase 3: Worktree Lifecycle | Complete | `worktree_enter`, `worktree_list`, and `worktree_exit` now exist with persisted worktree events, shared runtime context switching, child-agent dedicated worktree opt-in, and persisted task/worktree summaries that survive reload. |
| Phase 4: Checkpoint And Restore | Deferred | Rollback remains transcript/history-centric, and a durable checkpoint design has not been locked yet. |
| Phase 5: Diagnostics | Complete | `code_diagnostics` exists as a typed tool surface and no longer has a mirrored slash command. |
| Phase 6: Cron / Automation | Complete | `cron_create`, `cron_list`, and `cron_delete` now persist typed schedule/template state, restore on startup, stay session-scoped, and resume future runs after process restart. |
| Phase 7: Code Search | Complete | Canonical `code_search` now returns typed ranked matches with explicit scores; managed backends merge semantic workspace-symbol hits with lexical snippet fallback, while lexical-only hosts still expose deterministic index-backed ranking. |
| Phase 8: Browser / Computer Use | Complete | Feature-gated `browser_open`, `browser_snapshot`, `browser_click`, `browser_type`, `browser_eval`, `browser_screenshot`, and `browser_close` now cover typed browser session creation, DOM inspection, click/type/eval interactions, screenshot capture, and explicit session teardown. |
| Phase 9: Notebook Editing | Complete | Feature-gated `notebook_read` and `notebook_edit` now expose typed notebook inspection and mutation without falling back to raw `.ipynb` JSON tooling. |
| Cross-cutting: Operator Slash Surface | Complete | Built-in slash commands are now constrained to operator/session surfaces, `/new` owns the `clear` alias, installed skills remain the only deliberate `/skill_name` exception, and tests prevent canonical model tools from reappearing as built-in slash commands. |
| Cross-cutting: Tool Review Surface | Complete | `ToolReview` is now a typed item-based review substrate instead of a diff-only file list, and the TUI can open the same centered review overlay for running, failed, and completed tools with structured sections or file diffs. |
| Cross-cutting: Skill Lifecycle & Self-Evolution | In Progress | Hermes-style `skills_list`, `skill_view`, and `skill_manage` now exist with managed-vs-external roots, managed roots deterministically override readonly external copies, shadowed copies are surfaced as provenance, Hermes-style trust/update/audit metadata now flows through skill loading plus `skills_list` / `skill_view`, and managed skills now support archived snapshots plus typed restore; verifier-backed extraction and promotion are still missing. |

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
- code search/navigation primitives: `glob`, `grep`, `list`, `code_search`,
  `code_symbol_search`, `code_document_symbols`, `code_nav`
- execution and browsing: `exec_command`, `write_stdin`, `web_fetch`,
  `web_search`
- host-mediated coordination: `request_user_input`, `request_permissions`
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
   Execution objects such as tasks, monitors, checkpoints, and worktrees get
   their own tool families. Do not reintroduce a transcript-only plan surface
   as a second coordination model.

4. Ship each phase end-to-end.
   Each slice must include protocol, runtime wiring, persistence, TUI surface,
   and tests before moving on.

5. Do not build UI-only features without substrate support.
   If the operator can inspect, resume, stop, or restore something, the
   capability must exist as a typed backend/runtime contract first.

6. Keep model tools and operator commands as separate surfaces.
   Model-visible tools do not automatically get slash commands. Slash commands
   exist for session control, review, history, export, attachments, and
   runtime supervision. Skills are the deliberate exception: they are exposed
   as explicit operator invocations through both `/skill_name` and
   composer-native `$skill_name` directives.

7. Keep one execution model.
   If an object needs its own lifecycle, persistence, or operator controls, it
   must be modeled as a typed entity instead of being projected into transcript
   prose or an auxiliary plan surface.

## Phase 1: Task Model

### Objective

Introduce a first-class task object model so execution tracking stops leaking
into live-task side channels and transcript-only summaries.

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

- Remove `update_plan` instead of keeping a second coordination surface.
- Move live-task operator summaries to typed task records.
- Stop treating child-agent handles as the only execution object.

### Acceptance

- A task can exist without a child agent.
- A child agent can back a task without collapsing into transcript-only state.
- Task history survives session reload.
- `/tasks` and `/task` render typed task records, not reconstructed prose.

### Exit criteria

- The model manages task coordination exclusively through typed task tools.
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

### Current status

- Session-scoped worktree lifecycle now exists as a typed tool family:
  - `worktree_enter`
  - `worktree_list`
  - `worktree_exit`
- Entering or exiting a worktree updates the shared runtime tool context, so
  later tools in the same session inherit the active worktree root.
- Worktree lifecycle is persisted through typed session events and projected
  into transcript/history surfaces.
- `spawn_agent` now exposes typed `worktree_mode` input, and dedicated child
  worktrees are created and released through the shared worktree manager.
- Persisted task summaries now retain `worktree_id` and `worktree_root`, so
  task history survives reload without losing worktree ownership.
- Remaining follow-up is presentation-only:
  - richer operator review for worktree ownership and rollback

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
- Child-agent worktree ownership survives spawn, completion, and task reload.

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

### Current status

- `cron_create`, `cron_list`, and `cron_delete` are implemented behind the
  `automation-tools` feature.
- It supports one-shot delays and recurring `every_seconds` schedules.
- Each run materializes a typed `automation_backed` task and publishes a typed
  automation notification into the session stream.
- `cron_list` returns typed schedule summaries ordered by their next run.
- `cron_delete` cancels future runs while keeping a typed cancelled tombstone
  for later inspection.
- Created automations persist their full typed task template plus execution
  context, including attached worktree ownership when present.
- Startup now restores persisted schedules and resumes any non-terminal
  automation without waiting for an operator-side `cron_list`.
- `cron_list` and `cron_delete` are session-scoped and no longer leak
  schedules across sessions.

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

### Current status

- completed:
  - `browser_open` now exists behind the `browser-tools` feature
  - browser sessions persist typed `BrowserOpened` events and render through the
    same typed tool/TUI surfaces as other runtime-owned objects
  - `browser_snapshot` returns typed page text, interactive element summaries,
    and optional bounded HTML previews without falling back to generic JSON
  - `browser_click` resolves browser sessions through the same typed selection
    rules and persists click-driven `BrowserUpdated` summaries
  - `browser_type` adds clear/submit/navigation controls instead of splitting
    input automation across multiple ad-hoc tool names
  - `browser_eval` keeps promise handling explicit and returns typed JSON values
    instead of opaque browser protocol blobs
  - `browser_screenshot` returns a first-class PNG image part plus typed
    metadata instead of degrading screenshots into a local-path side channel
  - `browser_close` marks sessions as closed, removes them from the live
    browser registry, and persists the final typed `BrowserUpdated` summary

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
- Notebook tools stay behind the `notebook-tools` feature instead of widening
  the default model-visible surface.

### Exit criteria

- Common notebook workflows no longer require brittle raw JSON edits.
- Both notebook inspection and mutation are available as typed tool surfaces
  when `notebook-tools` is enabled.

## Cross-Cutting Workstream: Skill Lifecycle And Self-Evolution

### Objective

Bring skills onto the Hermes baseline first: explicit discovery, progressive
loading, managed-vs-external roots, and typed skill mutation through a single
skill-management surface. Only after that substrate exists should verifier-
backed extraction, archival, and promotion workflows be layered on top.

### User-facing outcome

- The runtime exposes Hermes-style skill tools:
  - `skills_list`
  - `skill_view`
  - `skill_manage`
- Skills live in a writable managed root plus optional external read-only
  roots, with managed skills taking precedence on name collisions.
- Skill invocation is explicit through both `$skill_name` and `/skill_name`,
  but the model must still discover/inspect skills through `skills_list` and
  `skill_view` instead of relying on prompt-injected catalogs.
- Updating a skill emits an explicit follow-up note in tool output instead of
  mutating the runtime instruction prefix.

### Planned work

- completed:
  - add managed vs external skill roots plus provenance metadata
  - add Hermes-style `skills_list`, `skill_view`, and `skill_manage`
  - remove prompt-manifest/system-preamble catalog injection
  - surface installed skills in the TUI as `/skill_name` slash commands while
    keeping `$skill_name` composer directives
  - surface Hermes-style provenance trust/update/audit metadata through typed
    skill loading and `skills_list` / `skill_view`
  - add managed archive/restore flow so skill mutations are reversible without
    treating workspace roots as the only source of truth
- next:
  - define verifier-backed extraction of reusable skills from successful runs
  - align tool-vs-skill guidance with the Hermes split:
    - use skills for instruction-plus-existing-tool workflows
    - use tools for capabilities that require typed runtime integration
  - add promotion policy on top of archived skill revisions instead of treating
    every archived snapshot as equally eligible for reuse

### Write set

- `crates/skills/`
- `crates/tools/src/agentic/skill.rs`
- `apps/code-agent/crates/backend/src/backend/`
- `apps/code-agent/crates/tui/src/frontend/tui/`
- `docs/meta-agent-evolution-plan.md`

### Exit criteria

- Hermes baseline is fully in place:
  - discovery via `skills_list`
  - progressive loading via `skill_view`
  - managed mutation via `skill_manage`
  - stable slash/composer skill invocation without prompt-catalog injection
- Reusable skill evolution beyond the Hermes baseline is auditable,
  reversible, and verifier-gated.

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

### Completed work

- generalized `ToolReview` from diff-only file previews to typed review items
  with `file_diff` vs structured section kinds
- taught transcript tool entries to retain reviewable structured input/output
  sections even when the tool did not emit file diffs
- kept file-diff review as the canonical surface for mutating tools while
  letting running/failed command, browser, and other typed tools reuse the
  same operator review overlay
- aligned the overlay UI around typed nouns (`files` vs `sections`) instead of
  hard-coding every review flow to diff/file semantics
- kept raw protocol markers such as `path>` / `query>` out of operator-facing
  review and transcript rendering whenever typed fields exist

### Exit criteria

- The transcript remains readable under concurrent tool activity.
- The operator can tell what ran, what is still running, and what failed
  without expanding raw payloads.
- The same review overlay can inspect both file diffs and structured
  input/output sections without falling back to raw JSON payload dumps.

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
- Do not force skills through only one interaction surface. Keep both
  `/skill_name` and `$skill_name` explicit and first-class.
- Do not encode new state machines as transcript strings.
- Do not reintroduce slash commands that merely proxy model tools when a typed
  picker action or direct model tool invocation already exists.

## Immediate Next Step

The next highest-value implementation slices are:

1. Phase 3: add typed worktree lifecycle
2. Phase 4: split transcript rewind from durable checkpoint restore
3. cross-cutting: finish the operator tool review surface for concurrent
   running/completed tool cells
4. cross-cutting: extend Hermes-baseline skill management into verifier-backed
   extraction and promotion flow
