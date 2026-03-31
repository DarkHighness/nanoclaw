# Code Agent Example

This app is the smallest codex-like code agent built on top of the `nanoclaw` foundation crates, with a
compact `ratatui` terminal UI that still feels like a real product surface.

It intentionally keeps the host layer thin:

- model-visible coding tools: `read`, `view_image`, `write`, `edit`, provider-specific patch tools (`apply_patch` on OpenAI, `patch` on Anthropic), `glob`, `grep`, `list`, `exec_command`, `write_stdin`
- discovery tools: `tool_search`, `tool_suggest`
- optional code-intel tools: `code_symbol_search`, `code_document_symbols`, `code_definitions`, `code_references`
- agentic tools: `update_plan`, `request_user_input`, `request_permissions`, `task`, `spawn_agent`, `send_input`, `wait_agent`, `resume_agent`, `list_agents`, `close_agent`
  - `spawn_agent` accepts Codex-style launch overrides such as `fork_context`, `model`, and `reasoning_effort`
  - `spawn_agent` and `send_input` now forward `message + items` as structured user messages instead of flattening them into steering prose
  - `send_input interrupt=true` now performs a real child restart instead of queuing behind the active turn, and the TUI/history surfaces distinguish queued follow-ups from interrupt-driven restarts
  - `local_image` and `image_url` input items now become first-class image parts, so multimodal child prompts reuse provider-native image transport instead of a text-only resource fallback
  - `local_file` now means a workspace file path, while `file` accepts either a workspace path or an `http/https` URL; both attach first-class file parts
  - OpenAI forwards file parts as `input_file`, while Anthropic upgrades PDFs to native `document` blocks and keeps other file types on a readable fallback path
  - `mention`, `skill`, and generic `item` inputs now travel as typed reference parts instead of being flattened into ad hoc resource metadata or JSON
  - content-oriented summaries now keep image and file placeholders in `Message::text_content()` instead of dropping attachment-only turns
  - transcript exports and MCP prompt/resource previews now reuse the same operator-visible message-part renderer instead of drifting across host-specific formatters
  - session search now keeps content-oriented matching separate from operator-visible preview rendering, so references and other structured parts search cleanly without losing their typed markers in UI previews
- append-only runtime loop from `runtime`
- runtime steering and queued command support
- loop detection as the primary guard against tool-call churn, without a fixed global iteration cap
- provider adapter from `provider`
- workspace skills loaded from conventional skill roots
- interactive approval for destructive tools
- backend-owned approval and runtime event contracts for frontend reuse
- streaming assistant output in a `ratatui` TUI
- manual and automatic context compaction
- persistent session history with replay and export commands
- MCP server, prompt, and resource inspection from the TUI
- backend-owned startup diagnostics surfaced through the inspector

## Custom Tools

Workspace-local custom tools can be dropped into `.nanoclaw/tools` as command
manifests. The host scans:

- `.nanoclaw/tools/*.toml`
- `.nanoclaw/tools/<tool_name>/tool.toml`

Plugins can export the same manifest contract by declaring
`components.tool_roots` in `.nanoclaw-plugin/plugin.toml`; those plugin tools
load through the same dynamic registry path with plugin-scoped sandbox grants.

Each manifest registers one dynamic tool backed by a local command. The command
receives the JSON invocation payload on stdin and can either print plain text or
return a JSON envelope with `text`, `structured_content`, `metadata`,
`continuation`, and `attachments`.

Minimal example:

```toml
description = "Summarize a workspace-specific checklist."
program = "./run.sh"

[[parameters]]
name = "topic"
type = "string"
description = "Checklist topic."
required = true

[approval]
read_only = true
mutates_state = false
idempotent = true
```

## Usage

Interactive REPL:

```bash
cp apps/code-agent/.env.example .env
# edit .env
cargo run --manifest-path apps/Cargo.toml -p code-agent
```

This opens a compact `ratatui` screen with a single wide main surface for
transcript and read-heavy command views, a Codex-style prompt line, a minimal
context footer, inline approval and user-input bands in the bottom pane, and a
more neutral dark palette tuned for prompt and slash-command workflows.

Use `/permissions` inside the TUI to inspect or switch the session base sandbox
mode between `default` and `danger-full-access`. Model-issued
`request_permissions` grants stay separate and layer on top of that base mode
for the current turn or session.

The bottom status line is configurable through `.nanoclaw/apps/code-agent.toml`.
By default it surfaces the current status, full model name plus reasoning
effort, current directory name, git repository and branch when available,
context-window usage, cumulative input/output tokens, queued command depth, and
local time.
Use `/statusline` inside the TUI to open a multi-select picker and toggle those
footer items on or off for the current operator session.

One-shot prompt:

```bash
cargo run --manifest-path apps/Cargo.toml -p code-agent -- "inspect this repository and explain the test layout"
```

The prompt is submitted as the first turn, then the TUI stays open.

## Environment

- The app automatically loads `.env` and `.env.local` from the current workspace.
- Precedence is: command-line flags > process environment > `.env.local` > `.env`.
- Shared core settings come from `.nanoclaw/config/core.toml` plus host/plugin-oriented `NANOCLAW_CORE_*` env overrides.
- App-local settings come from `.nanoclaw/apps/code-agent.toml`.
- Runtime thread caps can be set in `.nanoclaw/config/core.toml` with
  `host.tokio_worker_threads` and `host.tokio_max_blocking_threads`,
  or via `NANOCLAW_CORE_TOKIO_WORKER_THREADS` and
  `NANOCLAW_CORE_TOKIO_MAX_BLOCKING_THREADS`.
- `CODE_AGENT_LSP_ENABLED`: enable the managed LSP overlay for code-intel tools and file-open hooks (defaults to `true`)
- `CODE_AGENT_LSP_AUTO_INSTALL`: allow automatic installation of supported LSP servers into the managed cache (defaults to `false`)
- `CODE_AGENT_LSP_INSTALL_ROOT`: optional override for the managed LSP cache/install directory (defaults to `.nanoclaw/tools/lsp` under the workspace)
- `OPENAI_API_KEY` / `ANTHROPIC_API_KEY`: provider credentials
- `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL`: provider-specific API base URL overrides

Example app-local TUI settings:

```toml
[tui.statusline]
status = true
model = true
cwd = true
repo = true
branch = true
context_window = true
input_tokens = true
output_tokens = true
queue = true
clock = true
session = false
```

Set any field to `false` to hide it from the bottom status line.

Example:

```bash
cp apps/code-agent/.env.example .env
```

The code-agent now reads model selection from the shared model catalog and
agent profiles in `.nanoclaw/config/core.toml`. Provider credentials and
endpoint overrides stay provider-native:

- process environment
- `.env.local`
- `.env`

Use `OPENAI_API_KEY` / `OPENAI_BASE_URL` for OpenAI and `ANTHROPIC_API_KEY` /
`ANTHROPIC_BASE_URL` for Anthropic. The shared config now uses the
`global_system_prompt / host / models / agents / internal` layout, so the host
no longer reads legacy `provider`, `runtime`, or top-level `system_prompt`
sections.

If no skill roots are provided, it loads any existing directories from:

- `.codex/skills`
- `.nanoclaw/skills`
- `$HOME/.codex/skills`

The app now materializes the standard workspace state layout on startup:

- `.nanoclaw/logs`
- `.nanoclaw/store`
- `.nanoclaw/skills`
- `.nanoclaw/tools/lsp`
- `.nanoclaw/plugins`
- `.nanoclaw/apps`

## Managed LSP

`code-agent` now follows the same broad LSP lifecycle that `opencode` uses for its code-aware file tools:

- reading a supported source file triggers a best-effort `didOpen`
- write/edit/patch/apply_patch mutations trigger best-effort document sync so later semantic queries see fresh content
- code-intel tools reuse the same per-language server session instead of spawning one process per query

The difference is that `code-agent` also supports a managed install path when `CODE_AGENT_LSP_AUTO_INSTALL=true`.
If auto-install is disabled or the required package manager is unavailable, the code-intel tools fall back to the built-in lexical workspace index.

The implementation now keeps two layers separate on purpose:

- language recognition decides whether a file can participate in semantic sync and which `languageId` it should use
- server management decides whether the matching LSP can be started from `PATH`, auto-installed into the managed cache, or only used in fallback mode

Current managed install matrix:

- TypeScript / JavaScript: `typescript-language-server` via `npm`
- HTML: `vscode-html-language-server` via `npm`
- CSS / SCSS / Sass / Less: `vscode-css-language-server` via `npm`
- JSON / JSONC: `vscode-json-language-server` via `npm`
- Python: `python-lsp-server` (`pylsp`) via `python -m pip`
- Go: `gopls` via `go install`
- YAML: `yaml-language-server` via `npm`
- Shell: `bash-language-server` via `npm`
- Dockerfile / Containerfile: `docker-langserver` via `npm`
- PHP: `intelephense` via `npm`
- TOML: `taplo` via `cargo install`
- SQL: `sqls` via `go install`

Current auto-start-only matrix when the executable is already on `PATH`:

- Rust: `rust-analyzer`
- Java: `jdtls`
- C / C++ / Objective-C: `clangd`

Current recognized file/language matrix also covers a few editor-important filename cases that do
not have useful extensions, including `Dockerfile*`, `Containerfile*`, `go.mod`, `go.sum`,
`go.work`, and common shell rc files such as `.bashrc` and `.zshrc`.

## Commands

- `/status`
- `/statusline`
- `/thinking [level]`
- `/help`
- `/details`
- `/queue`
- `/agent_sessions [session-ref]`
- `/agent_session <agent-session-ref>`
- `/live_tasks`
- `/spawn_task <role> <prompt>`
- `/send_task <task-or-agent-ref> <message>`
- `/wait_task <task-or-agent-ref>`
- `/cancel_task <task-or-agent-ref> [reason]`
- `/tasks [session-ref]`
- `/task <task-id>`
- `/sessions [query]`
- `/session <session-ref>`
- `/resume <agent-session-ref>`
- `/export_session <session-ref> <path>`
- `/export_transcript <session-ref> <path>`
- `/tools`
- `/skills`
- `/diagnostics`
- `/mcp`
- `/prompts`

## TUI shortcuts

- `Ctrl+T` cycles the active model reasoning effort through the levels exposed by the active model.
- While a turn is running, `Enter` schedules a safe-point steer note and `Tab`
  enqueues a follow-up prompt into the runtime-owned control queue.
- While a turn is running, `Esc` interrupts immediately. If pending steers
  exist, all pending steers are merged in FIFO order into the next user
  message; otherwise the composer returns to an idle "what next?" state.
- While idle with an empty composer, `Esc` arms history rollback. Press `Esc`
  again to open the rollback overlay, `Esc`/`Left` to move to older user turns,
  `Right` to move forward, `Enter` to confirm, and `q` to cancel. Confirming a
  rollback removes the selected user turn and everything after it, then restores
  that user prompt into the composer for editing.
- The composer now keeps a workspace-persistent text history plus an in-memory
  session-local draft history. `Up` / `Down` recall prompt history only when the
  cursor is already at a buffer boundary; otherwise those keys move vertically
  through multiline drafts and only fall back to the start or end of the draft
  when there is no adjacent line. `Left`, `Right`, `Home`, and `End` now edit
  inside the prompt instead of always acting on pane scroll state.
- `Ctrl+K` kills the draft tail from the cursor to end-of-line into a local kill
  buffer, and `Ctrl+Y` yanks it back. The kill buffer retains draft attachments
  such as large-paste payloads, so yanking after a clear or submit restores the
  full text.
- `Ctrl+C` now clears the current draft into session-local composer history when
  the prompt line is non-empty, so `Up` can restore it. On an empty prompt line,
  `Ctrl+C` still exits the TUI.
- Very large pasted payloads are now collapsed into `[Paste #N]` placeholders in
  the composer. The full payload stays in session-local draft attachment state,
  is submitted as a typed message part while persistent history stays plain
  text, and is restored when recalling a stashed draft.
- `Alt+Up` opens the pending-control picker from the bottom pane.
- In the pending-control picker: `Enter` edits the selected item, `Delete`
  withdraws it, and `Esc` closes the picker.
- `/resources`
- `/prompt <server> <name>`
- `/resource <server> <uri>`
- `/steer <notes>`
- `/new`
- `/compact [notes]`
- `/clear` (alias of `/new`)
- `/exit`

The product-facing host surface now uses `session` terminology for the durable
conversation history and `agent session` terminology for runtime-resume targets.
`/session <session-ref>` opens persisted conversation history and exports
artifacts. `/agent_session <agent-session-ref>` inspects a specific runtime
window, including its transcript slice, token budget, and spawned subagent
summaries. `/spawn_task <role> <prompt>` launches a new live child task from
the active top-level session, `/live_tasks` lists currently attached child
agents for that runtime, `/send_task <task-or-agent-ref> <message>` sends a
follow-up user message to a live child, `/wait_task <task-or-agent-ref>` waits
for one child in the background, and `/cancel_task <task-or-agent-ref> [reason]`
can stop one without leaving the current session. `/tasks [session-ref]` lists
persisted child tasks, and `/task <task-id>` opens their prompt/result/artifact
view plus the child session transcript. `/resume <agent-session-ref>` resolves
an
`AgentSessionId` instead of a top-level `SessionId`. Historical agent sessions
can now be reattached into the live runtime, and the resumed runtime receives a
fresh active `AgentSessionId` bound to the original top-level `SessionId`.
Older compacted histories that predate resume checkpoints still remain
history-only.

`/thinking` opens a picker when invoked without an explicit level. Hosts can
also declare model-specific effort support in the core model config:

```toml
[models.gpt_5_4_default]
provider = "openai"
model = "gpt-5.4"
reasoning_effort = "medium"
supported_reasoning_efforts = ["low", "medium", "high"]
```

`/new` and `/clear` now perform the same backend-owned operation: they create a
fresh active top-level session while keeping prior sessions in durable history
for browsing, export, and later reattachment.

Those lifecycle actions now flow through a typed backend session-operation
contract, so future frontends do not need to orchestrate separate calls for
reset, resume, startup refresh, and transcript reload.

The live child-agent operator surface is now backend-owned as well: the TUI no
longer needs direct runtime access just to spawn, inspect, steer, wait on, or
cancel currently attached subagents. Those host-originated live-task actions
now anchor themselves to the active top-level `SessionId` / `AgentSessionId`,
so the durable task log can reconstruct operator-created child work later.
`wait` runs as a background operator task so the TUI can keep rendering runtime
events while the selected child agent finishes.

The startup inspector is now backed by a structured backend snapshot, and the
MCP-focused commands expose connected server catalogs plus prompt/resource
loading directly from `code-agent` itself.

The TUI now follows a more minimal shell: a single main pane for transcript and
command views, a bottom context footer, and a compact prompt line. When LSP or
TODO context is available on wide terminals, the shell adds only a narrow side
rail for brief context. The palette is intentionally muted rather than
blue-accented, transcript turns are separated visually, and read-heavy outputs
such as `/help`, command catalogs, and history lists now open in the main
pane. The main transcript surface intentionally leaves terminal-native mouse
selection enabled, so copy workflows still work with the operator's existing
terminal emulator instead of requiring a host-specific clipboard command.

Tool-heavy transcript entries now default to a collapsed shell summary so the
main timeline stays readable. `/details` toggles the full tool payload stream
back on when an operator needs to inspect command previews, structured output,
or diff blocks inline.

Collection-heavy views such as session, agent-session, task, and search lists
now stay tighter as well: duplicated pane titles are suppressed, list headers
render once, and the session-search / agent-session summaries stay on a compact
two-line shell layout instead of spreading metadata across extra blank rows.

Transcript rendering is now closer to Codex's own TUI implementation: user
turns use `›`, assistant and runtime summaries use `•`, approvals resolve into
`✔` or `✗` history lines, and live runtime progress stays inline in the
conversation instead of being duplicated into visible `tool>` / `approval>` /
`model>` tags.

Markdown-heavy assistant output now renders through a syntax-aware pipeline
instead of the older ad hoc formatter. Fenced code blocks, `diff` snippets, and
common Markdown structure all render directly in the transcript, and file
mutation tools such as `write`, `edit`, and `patch` now surface structured diff
previews instead of only terse completion summaries.

Approval prompts now render as compact bottom-pane questions with inline
command previews instead of large modal panels. The bottom status line also now
carries the current runtime status, detail-toggle state, queue depth, and
session context in one structured row instead of a thin context-only footer.

Interactive approval and live runtime updates now also route through
backend-owned contracts, so the TUI renders session events and approval prompts
without constructing runtime observers or approval handlers on its own.
