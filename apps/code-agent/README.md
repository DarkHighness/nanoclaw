# Code Agent Example

This app is the smallest codex-like code agent built on top of the `nanoclaw` foundation crates, with a
compact `ratatui` terminal UI that still feels like a real product surface.

It intentionally keeps the host layer thin:

- model-visible coding tools: `read`, `write`, `edit`, `patch_files`, `glob`, `grep`, `list`, `exec_command`, `write_stdin`
- discovery tools: `tool_discover`
- operator/debug-only tools: `web_search_backends`
- optional code-intel tools: `code_search`, `code_symbol_search`, `code_document_symbols`, `code_nav`
- agentic tools: `skills_list`, `skill_view`, `skill_manage`, `request_user_input`, `request_permissions`, `task_create`, `task_get`, `task_list`, `task_update`, `task_stop`, `spawn_agent`, `send_input`, `wait_agent`, `resume_agent`, `list_agents`, `close_agent`
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
- Hermes-style skill discovery and mutation:
  - `skills_list` discovers the catalog without injecting it into the system prompt
  - `skill_view` progressively loads `SKILL.md` content or companion files on demand
  - `skill_manage` mutates only the managed root and tells the model to refresh via `skills_list`/`skill_view`
- interactive approval for destructive tools and higher-risk external reads,
  with a narrow host allowlist for safe built-in web research tools,
  argv-matched `exec_command` trust rules, approval-free `write_stdin`
  follow-ups, and
  transport-aware MCP resource reads
- backend-owned approval and runtime event contracts for frontend reuse
- hook-emitted live TUI cues (`show_toast`, `append_prompt`) projected through
  the runtime observer plane instead of synthetic transcript text
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

The default build excludes the heavier `memory-embed` backend. Re-enable it
when needed with:

```bash
cargo run --manifest-path apps/Cargo.toml -p code-agent --features memory-embed
```

This opens a compact `ratatui` screen with a single wide main surface for
transcript and read-heavy command views, a Codex-style prompt line, a minimal
context footer, inline approval and user-input bands in the bottom pane, and a
more neutral dark palette tuned for prompt and slash-command workflows.

Installed skills are available through both `$skill_name` in the composer and
auto-generated `/skill_name` slash commands. The slash surface remains
operator-oriented for host/session actions, but explicit skill invocation is a
first-class exception so operators can browse and seed skill prompts without
mirroring arbitrary model tools into slash commands.

Use `/permissions` inside the TUI to inspect or switch the session base sandbox
mode between `default` and `danger-full-access`. Model-issued
`request_permissions` grants stay separate and layer on top of that base mode
for the current turn or session.

That sandbox toggle is separate from host approval policy. It does not disable
approval prompts by itself. `code-agent` now derives host-side approval
relaxation from app-local approval config instead of baking those choices into
the policy implementation.

`code-agent` can also remember host-local trusted exec rules through
`.nanoclaw/apps/code-agent.toml`. These rules are intentionally narrow. They
only apply to built-in local `exec_command` calls whose raw shell string stays a
single simple command without shell control syntax such as pipes, redirects,
command substitution, chained commands, or newlines. The host parses that
simple command into argv tokens and matches either an exact argv sequence or an
argv prefix rule. Nested shells such as `bash -lc ...` and inline interpreter
entrypoints such as `python -c ...` stay on the normal approval path. `write_stdin`
does not open a second approval step. Harmfulness is decided on `exec_command`,
and stdin follow-ups stay inside that existing session.

Typed `task_*` coordination is approval-free. These tools mutate host-owned
task state rather than the workspace or an external system, so they do not
share the same approval path as filesystem writes or new process execution.
The TUI and history surfaces now track typed task records directly instead of
projecting a separate high-level plan surface.

MCP resource reads now use the connected server boundary instead of treating
every MCP resource as the same risk. Resources from locally launched `stdio`
servers stay on the trusted local-process path, while `streamable_http` MCP
resources still fall back to the normal approval flow.

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

Example host-local approval settings:

```toml
[approval]
default_mode = "ask"
auto_allow_builtin_local_tool_names = ["web_search", "web_fetch"]
auto_allow_local_stdio_mcp_resource_reads = true

[[approval.rules]]
effect = "deny"
reason = "block remote pushes"
tool_names = ["exec_command"]

[approval.rules.exec]
argv_exact = ["git", "push"]

[approval.exec]

[[approval.exec.rules]]
argv_prefix = ["git", "status"]

[[approval.exec.rules]]
argv_exact = ["cargo", "test", "-p", "store"]
```

These rules match only simple shell commands after tokenization. A rule like
`argv_prefix = ["git", "status"]` can auto-approve `git status --short`, but
`git status; rm -rf .`, `bash -lc 'git status'`, and `python -c '...'` still
fall back to the normal approval flow.

The top-level `[approval]` section controls which built-in local tools stay on
the host allowlist and whether transport-aware local `stdio` MCP resource reads
should skip approval. `default_mode` lets the host ask or deny unmatched tool
calls without changing runtime-wide behavior. Explicit `[[approval.rules]]`
entries are evaluated in order before the compatibility fields above. Remote
`streamable_http` MCP resources still stay on the normal approval path.

The older `auto_allow_*` and `[approval.exec]` fields are still accepted as
compatibility sugar. `code-agent` lowers them into the same ordered host rule
model that powers explicit `approval.rules`.

Model-visible mutation exposure is now normalized around one staged multi-file
surface. `code-agent` keeps `write` and `edit` for single-file work and exposes
`patch_files` as the canonical multi-file mutator. `patch_files` now carries
both structured operations and an optional freeform patch grammar, so hosts can
project one capability through either function or custom-tool transport without
keeping extra legacy tool names alive.

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
- write/edit/patch_files mutations trigger best-effort document sync so later semantic queries see fresh content
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
- `/monitors [all]`
- `/stop_monitor <monitor-ref> [reason]`
- `/tasks [session-ref]`
- `/task <task-id>`
- `/sessions [query]`
- `/session <session-ref>`
- `/resume <agent-session-ref>`
- `/export_session <session-ref> <path>`
- `/export_transcript <session-ref> <path>`
- `/diagnostics`
- `/mcp`
- `/prompts`
- `/resources`

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
- `Ctrl+O` opens `$VISUAL` or `$EDITOR` with the current composer text. When
  remote attachment rows are present, the editor seed includes an
  `[Attachments]` section ahead of `[Prompt]`, so removing or reordering those
  placeholder lines detaches or reorders the pending remote image/file rows.
  Saving and closing reapplies the edited prompt text, drops missing inline
  placeholders, and rebinds surviving local attachment placeholders plus large
  paste placeholders into stable `[Image #N]`, `[File #N]`, and `[Paste #N]`
- In command and history views, `Up` / `Down` move the focused list row. `Enter`
  opens or runs the selected item. Agent-session rows that are still
  reattachable also expose `r` to resume directly from the list without typing
  `/resume <agent-session-ref>` by hand.
  labels.
- `Ctrl+C` now clears the current draft into session-local composer history when
  the prompt line is non-empty, so `Up` can restore it. On an empty prompt line,
  `Ctrl+C` still exits the TUI.
- Very large pasted payloads are now collapsed into `[Paste #N]` placeholders in
  the composer. The full payload stays in session-local draft attachment state,
  is submitted as a typed message part while persistent history stays plain
  text, and is restored when recalling a stashed draft.
- `/image <path-or-url>` and `/file <path-or-url>` add first-class composer
  attachments. Local workspace paths are inserted into the prompt as inline
  placeholders such as `[Image #1]` and `[File #1]`, while remote
  `http/https` attachments stay as rows above the prompt line so they can be
  selected and reordered separately. `/detach [index]` removes the latest row
  or a specific numbered row. Those rows and inline placeholders both stay
  inside session-local draft history so recalled drafts restore the same
  pending attachments. `Up` at cursor position `0` selects the last row,
  `Up`/`Down` moves across rows, `Down` on the final row returns to text
  editing, `Delete`/`Backspace` removes the selected row, and
  `/move_attachment <from> <to>` reorders rows explicitly.
- Image attachments are gated by the active model surface. `/image` refuses to
  attach on non-vision models, prompt submit blocks before clearing the draft
  when pending image parts are present, and pasting a single local image path
  auto-attaches it instead of inserting raw text when the active model accepts
  image input.
- MCP prompt/resource picks now restore richer composer drafts too, so loaded
  images, files, and inline paste
  placeholders come back as first-class draft attachments instead of being
  flattened into plain text only.
- `Alt+Up` opens the pending-control picker from the bottom pane.
- In the pending-control picker: `Enter` edits the selected item, `Delete`
  withdraws it, and `Esc` closes the picker.
- `/image <path-or-url>`
- `/file <path-or-url>`
- `/detach [index]`
- `/move_attachment <from> <to>`
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
summaries. `/live_tasks` lists currently attached child agents for the active
runtime and exposes typed picker actions: `Enter` waits on the selected child,
and `c` cancels it without leaving the current session. `/tasks [session-ref]`
lists persisted child tasks, and `/task <task-id>` opens their
prompt/result/artifact view plus the child session transcript. `/resume
<agent-session-ref>` resolves an `AgentSessionId` instead of a top-level
`SessionId`. Historical agent sessions can now be reattached into the live
runtime, and the resumed runtime receives a fresh active `AgentSessionId`
bound to the original top-level `SessionId`. Older compacted histories that
predate resume checkpoints still remain history-only.

When a background live-task wait finishes, the TUI still records a transcript-side
completion notice, raises a short-lived toast, and surfaces an operator hint.
The host now also promotes that completion into runtime-owned control so the
model can react without waiting for a manual follow-up: if the main turn is
still running, Code Agent schedules a safe-point steer; if the runtime is idle,
it queues a synthetic user prompt and immediately drains it. Both the operator
cue and the model-visible follow-up include any still-running background tasks
so the assistant can remember which subagents remain in flight.

Runtime notification events now flow through the same live path too, so
loop-detector warnings and provider-state degradation notices can raise toasts
instead of remaining history-only diagnostics.

The same event bus now exposes a minimal OpenCode-style TUI surface too:
`tui.toast.show` raises transient notifications and `tui.prompt.append` can seed
composer follow-ups for host- or plugin-driven operator cues. Model-visible
background-task attention now routes through runtime steer/queue primitives
instead of relying on a human to submit a seeded draft.

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
MCP-focused commands expose connected server catalogs while prompt/resource
loading runs from typed picker actions instead of direct slash-command mirrors.

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
mutation tools such as `write`, `edit`, and `patch_files` now surface structured diff
previews instead of only terse completion summaries.

Approval prompts now render as centered modal overlays with labeled context,
request, reason, and key rows instead of sharing space with the bottom prompt
band. The command catalog also opens as a centered modal-style view, while
history and other collection-heavy views now use selectable two-line cards with
inline keyboard hints so opening sessions, inspecting agent sessions, and
resuming reattachable runtimes all happen directly from the list. The old live
side rail has been retired so plan / execution context reads as dedicated
transcript system cells instead of competing with the main timeline in a
parallel column.

Interactive approval and live runtime updates now also route through
backend-owned contracts, so the TUI renders session events and approval prompts
without constructing runtime observers or approval handlers on its own.
