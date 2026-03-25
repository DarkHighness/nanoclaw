# Code Agent Example

This app is the smallest codex-like code agent built on top of the `nanoclaw` substrate, with a
compact `ratatui` terminal UI that still feels like a real product surface.

It intentionally keeps the host layer thin:

- core coding tools: `read`, `write`, `edit`, `patch`, `glob`, `grep`, `list`, `bash`
- agentic tools: `todo_read`, `todo_write`, `task`
- append-only runtime loop from `agent-core-runtime`
- runtime steering and queued command support
- loop detection as the primary guard against tool-call churn, without a fixed global iteration cap
- provider adapter from `agent-core-rig`
- workspace skills loaded from conventional skill roots
- interactive approval for destructive tools
- streaming assistant output in a `ratatui` TUI
- manual and automatic context compaction

## Usage

Interactive REPL:

```bash
cp apps/code-agent-example/.env.example .env
# edit .env
cargo run --manifest-path apps/code-agent-example/Cargo.toml
```

This opens a `ratatui` screen with a branded header, conversation pane, inspector panel,
activity feed, queued-command status, and composer.

One-shot prompt:

```bash
cargo run --manifest-path apps/code-agent-example/Cargo.toml -- "inspect this repository and explain the test layout"
```

The prompt is submitted as the first turn, then the TUI stays open.

Explicit provider:

```bash
cargo run --manifest-path apps/code-agent-example/Cargo.toml -- --provider anthropic
```

## Environment

- The app automatically loads `.env` and `.env.local` from the current workspace.
- Precedence is: command-line flags > process environment > `.env.local` > `.env`.
- `CODE_AGENT_PROVIDER`: `openai` or `anthropic` (defaults to `openai`)
- `CODE_AGENT_SYSTEM_PROMPT`: extra system prompt text appended to the built-in preamble
- `CODE_AGENT_SKILL_ROOTS`: optional path-list of additional skill roots
- `OPENAI_API_KEY` / `ANTHROPIC_API_KEY`: provider credentials
- `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL`: provider-specific API base URL overrides

Example:

```bash
cp apps/code-agent-example/.env.example .env
```

The built-in model defaults are:

- OpenAI: `gpt-5.4`
- Anthropic: `claude-sonnet-4-6`

The app only chooses the provider at the app-config layer. Model selection is fixed in code per
provider, and API credentials and endpoint overrides stay provider-native:

- process environment
- `.env.local`
- `.env`

In other words, use `OPENAI_API_KEY` / `OPENAI_BASE_URL` for OpenAI and
`ANTHROPIC_API_KEY` / `ANTHROPIC_BASE_URL` for Anthropic. The example does not define its own
generic `MODEL` or `BASE_URL` env layer. At startup it injects `.env` values into the process
environment before building the runtime, so the provider adapter can consume the provider-native
variables directly.

If no skill roots are provided, it loads any existing directories from:

- `.codex/skills`
- `.agent-core/skills`
- `$HOME/.codex/skills`

## Commands

- `/help`
- `/tools`
- `/skills`
- `/steer <notes>`
- `/compact [notes]`
- `/clear`
- `/exit`
