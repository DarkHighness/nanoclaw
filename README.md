# nanoclaw

`nanoclaw` is a Rust workspace for building a provider-agnostic agent substrate.

Core crates:

- `agent-core-types`: shared protocol types
- `agent-core-runtime`: generic agent turn loop
- `agent-core-tools`: core local tool abstractions and built-ins
- `agent-core-skills`: skill package loading and cataloging
- `agent-core-rig`: provider adapters built on `rig-core`
- `agent-core-mcp`: MCP integration surface
- `agent-core-store`: persistence and replay surface

The repository also keeps a removable reference shell in `apps/agent-core-tui`. That crate is maintained independently from the substrate workspace and is one example of a host application shape, not part of the base framework contract.

## Framework Boundary

The intended boundary is:

- substrate closure: `agent-core-types`, `agent-core-runtime`, `agent-core-tools`, `agent-core-skills`, plus one provider adapter such as `agent-core-rig`
- core built-in tools: `read`, `write`, `edit`, `glob`, `grep`, `list`, `bash`
- optional tool bundles: non-essential tools such as first-party web access and agentic task/todo tools compile only behind Cargo features
- integration surfaces: `agent-core-mcp` and `agent-core-store` bolt onto the same runtime contracts
- reference product layer: `apps/agent-core-tui` sits outside the substrate, keeps its shell-local config private, and can be removed without changing the runtime core

The root workspace defaults follow that split:

- `cargo test` and `cargo check` target the substrate-oriented crates by default
- the reference shell is built and tested through its own manifest path instead of the substrate workspace

## Base Composition

The primary integration path is Rust code, not TOML. A host application should assemble the runtime explicitly:

```rust
use std::sync::Arc;

use agent_core::{
    AgentRuntimeBuilder, HookRunner, InMemoryRunStore, ReadTool, SkillCatalog,
    ToolExecutionContext, ToolRegistry, WriteTool,
};
use agent_core::rig::{RigBackendDescriptor, RigModelBackend, RigProviderDescriptor};

let cwd = std::env::current_dir()?;
let store = Arc::new(InMemoryRunStore::new());
let backend = Arc::new(RigModelBackend::from_descriptor(
    RigBackendDescriptor::new(RigProviderDescriptor::openai("gpt-4.1-mini")),
)?);

let mut tools = ToolRegistry::new();
tools.register(ReadTool::new());
tools.register(WriteTool::new());

let system_preamble = vec![
    "You are a general-purpose software agent operating inside the current workspace.".to_string(),
    "Inspect available state and use tools before guessing.".to_string(),
];

let runtime = AgentRuntimeBuilder::new(backend, store)
    .hook_runner(Arc::new(HookRunner::default()))
    .tool_registry(tools)
    .tool_context(ToolExecutionContext {
        workspace_root: cwd,
        workspace_only: true,
        ..Default::default()
    })
    .instructions(system_preamble)
    .skill_catalog(SkillCatalog::default())
    .build();
```

A compiled substrate-only example lives at [minimal_runtime.rs](/Users/twiliness/nanoclaw/crates/agent-core/examples/minimal_runtime.rs). It builds the preamble in Rust, wires a skill catalog, registers only the core tools, and runs the runtime without any TOML or TUI layer.

```bash
cargo run -p agent-core --example minimal_runtime
```

## Substrate Status

The core workspace now provides:

- append-only transcript history with hook output appended as messages instead of being reinserted ahead of prior context
- context compaction that summarizes only the older visible prefix while keeping a recent raw tail
- runtime-level steer and queue primitives for host-controlled session coordination
- loop detection for repeated tool-call churn, without a fixed global iteration cap in the runtime API
- stable `message_id` and `call_id` fields across provider events, tool calls, and persisted run history
- runtime-level tool approval hooks rather than shell-specific hardcoding
- a deterministic sorted tool registry
- skills as first-class catalog assets and hook carriers, without prompt-string activation heuristics
- provider streaming through the runtime observer boundary
- MCP `stdio` integration tests and provider-adapter contract tests
- feature-gated non-core tools such as `todo_read`, `todo_write`, `task`, `web_search`, and `web_fetch`

## Reference Shell

`apps/agent-core-tui` is a removable host application around the same runtime APIs. It exists to exercise the substrate end to end, not to define the framework, and it is maintained outside the root workspace.

Useful commands:

```bash
cargo run --manifest-path apps/agent-core-tui/Cargo.toml
cargo run --manifest-path apps/agent-core-tui/Cargo.toml --features web-tools
```

Inside the reference shell, operator commands include:

```text
/status
/compact [notes]
/runs
/runs <query>
/run <id-prefix>
/export_run <id-prefix> <path>
/export_transcript <id-prefix> <path>
/skills [query]
/skill <name-or-alias>
/tools
/mcp
/prompts
/prompt <server> <name>
/resources
/resource <server> <uri>
```

## Reference Shell Configuration

The file/env config layer applies only to the removable reference shell, and it now lives as a private module inside that shell crate.

`apps/agent-core-tui` reads:

- `agent-core.toml`
- `.agent-core/config.toml`
- `.env`
- `.env.local`
- process environment variables

Key shell-level knobs:

- `provider.kind`, `provider.model`, `provider.base_url`
- `provider.temperature`, `provider.max_tokens`, `provider.additional_params`
- `system_prompt`
- `skill_roots`
- `mcp_servers`
- `runtime.workspace_only`
- `runtime.auto_compact`
- `runtime.context_tokens`
- `runtime.compact_trigger_tokens`
- `runtime.compact_preserve_recent_messages`
- `runtime.store_dir`
- `hook_env`

Examples:

- shell config template: [agent-core.toml.example](/Users/twiliness/nanoclaw/apps/agent-core-tui/examples/agent-core.toml.example)
- OpenAI example: [openai example](/Users/twiliness/nanoclaw/apps/agent-core-tui/examples/openai/agent-core.toml)
- Anthropic example: [anthropic example](/Users/twiliness/nanoclaw/apps/agent-core-tui/examples/anthropic/agent-core.toml)

## Example Code Agent

For a thinner codex-like code agent example, see [apps/code-agent-example](/Users/twiliness/nanoclaw/apps/code-agent-example/README.md).

```bash
export OPENAI_API_KEY=...
cargo run --manifest-path apps/code-agent-example/Cargo.toml
```

## Documentation

- architecture: [docs/design.md](/Users/twiliness/nanoclaw/docs/design.md)
- design plan: [docs/plan.md](/Users/twiliness/nanoclaw/docs/plan.md)
- tooling research: [docs/tooling-research.md](/Users/twiliness/nanoclaw/docs/tooling-research.md)

## Testing

Substrate-focused default test run:

```bash
cargo test
```

Independent reference-shell test run:

```bash
cargo test --manifest-path apps/agent-core-tui/Cargo.toml
```

Targeted regressions:

```bash
cargo test -p agent-core --example minimal_runtime
cargo test -p agent-core-mcp --test stdio_integration
cargo test -p agent-core-rig --lib
```
