# Code Agent Example

This app is the smallest codex-like code agent built on top of the `nanoclaw` foundation crates, with a
compact `ratatui` terminal UI that still feels like a real product surface.

It intentionally keeps the host layer thin:

- core coding tools: `read`, `write`, `edit`, `patch`, `glob`, `grep`, `list`, `bash`
- optional code-intel tools: `code_symbol_search`, `code_document_symbols`, `code_definitions`, `code_references`
- agentic tools: `todo_read`, `todo_write`, `task`
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

## Usage

Interactive REPL:

```bash
cp apps/code-agent/.env.example .env
# edit .env
cargo run --manifest-path apps/Cargo.toml -p code-agent
```

This opens a `ratatui` screen with a branded header, conversation pane, inspector panel,
activity feed, queued-command status, and composer.

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
- write/edit/patch mutations trigger best-effort document sync so later semantic queries see fresh content
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
- `/help`
- `/sessions [query]`
- `/session <session-ref>`
- `/resume <session-ref>`
- `/export_session <session-ref> <path>`
- `/export_transcript <session-ref> <path>`
- `/tools`
- `/skills`
- `/diagnostics`
- `/mcp`
- `/prompts`
- `/resources`
- `/prompt <server> <name>`
- `/resource <server> <uri>`
- `/steer <notes>`
- `/compact [notes]`
- `/clear`
- `/exit`

The product-facing host surface now uses `session` terminology to match Codex,
Claude Code, and OpenCode. The durable history backend still stores entries by
substrate `run_id`, so `/session <session-ref>` currently opens persisted
history and exports artifacts, but it does not yet resume a live runtime from
that stored state.

The startup inspector is now backed by a structured backend snapshot, and the
MCP-focused commands expose connected server catalogs plus prompt/resource
loading directly from `code-agent` without relying on the legacy
`reference-tui` shell.

Interactive approval and live runtime updates now also route through
backend-owned contracts, so the TUI renders session events and approval prompts
without constructing runtime observers or approval handlers on its own.
