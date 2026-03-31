# nanoclaw

`nanoclaw` is a Rust repository with two Cargo workspaces:

- `crates/Cargo.toml`: the foundation workspace
- `apps/Cargo.toml`: removable host applications and examples

Foundation crates:

- `types`: shared protocol types
- `runtime`: generic agent turn loop
- `tools`: core local tool abstractions and built-ins
- `skills`: skill package loading and cataloging
- `provider`: provider adapters
- `mcp`: MCP integration surface
- `store`: persistence and replay surface
- `agent`: umbrella crate in `crates/core` that re-exports the foundation surface

The repository keeps its delivered host application in the separate `apps/`
workspace. `apps/code-agent` is the sole actively maintained product layer.

## Framework Boundary

The intended boundary is:

- minimal foundation closure: `types`, `runtime`, `tools`, `skills`, plus one provider adapter such as `provider`
- core built-in tools: `read`, `write`, `edit`, `patch`, `glob`, `grep`, `list`, `exec_command`, `write_stdin`
- optional tool bundles: non-essential tools such as first-party web access, code-intel navigation tools, and agentic task/todo tools compile only behind Cargo features
- integration surfaces: `mcp` and `store` bolt onto the same runtime contracts
- example product layer: `apps/code-agent` sits outside the foundation workspace, owns the product-facing host shell, and is the sole delivered app

The repository no longer treats the whole tree as one Cargo workspace. Foundation and app validation run through their own workspace manifests.

The directory layout follows that split directly:

- `crates/core`, `crates/runtime`, `crates/tools`, `crates/provider`, `crates/mcp`, `crates/store`, `crates/skills`, `crates/types`
- `apps/code-agent`

Every workspace now also has a standard mutable-state layout under `.nanoclaw/`:

- `logs/`: host tracing output
- `store/`: session persistence
- `skills/`: workspace-local skill packs
- `tools/lsp/`: managed language-server cache and install roots
- `plugins/`: workspace-local plugin bundles
- `apps/`: app-specific state that should stay isolated from the shared substrate roots

The umbrella crate keeps the Rust package name `agent` even though its directory is `crates/core`, because a package literally named `core` would collide with Rust's standard `core` crate.

## Base Composition

The primary integration path is Rust code, not TOML. A host application should assemble the runtime explicitly:

```rust
use std::sync::Arc;

use agent::{
    AgentRuntimeBuilder, HookRunner, InMemorySessionStore, ReadTool, SkillCatalog,
    ToolExecutionContext, ToolRegistry, WriteTool,
};
use agent::provider::{ProviderBackend, ProviderDescriptor};

let cwd = std::env::current_dir()?;
let store = Arc::new(InMemorySessionStore::new());
let backend = Arc::new(ProviderBackend::new(ProviderDescriptor::openai("gpt-5.4"))?);

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

A compiled foundation-only example lives at [minimal_runtime.rs](/Users/twiliness/nanoclaw/crates/core/examples/minimal_runtime.rs). It builds the preamble in Rust, wires a skill catalog, registers only the core tools, and runs the runtime without any TOML or TUI layer.

```bash
cargo run --manifest-path crates/Cargo.toml -p agent --example minimal_runtime
```

## Foundation Status

The core workspace now provides:

- append-only transcript history with hook output appended as messages instead of being reinserted ahead of prior context
- context compaction that summarizes only the older visible prefix while keeping a recent raw tail
- runtime-level steer and queue primitives for host-controlled session coordination
- loop detection for repeated tool-call churn, without a fixed global iteration cap in the runtime API
- stable `message_id` and `call_id` fields across provider events, tool calls, and persisted session history
- runtime-level tool approval hooks rather than shell-specific hardcoding
- a deterministic sorted tool registry
- skills as first-class catalog assets and hook carriers, without prompt-string activation heuristics
- provider streaming through the runtime observer boundary
- MCP `stdio` integration tests and provider-adapter contract tests
- feature-gated non-core tools such as `todo_read`, `todo_write`, `task`, `web_search`, and `web_fetch`
- feature-gated code-intel tools for symbol navigation: `code_symbol_search`, `code_document_symbols`, `code_definitions`, `code_references`
- grounded file mutations with `expected_snapshot` / `expected_selection_hash` guards across `write`, `edit`, `patch`, and `todo_write`

## App Workspace

`apps/code-agent` is the only host application kept in the active workspace.
Historical shell experiments now live only in archived design material and do
not define the repository's delivery shape.

## Example Code Agent

The primary example app is [apps/code-agent](/Users/twiliness/nanoclaw/apps/code-agent/README.md).

```bash
export OPENAI_API_KEY=...
cargo run --manifest-path apps/Cargo.toml -p code-agent
```

## Documentation

- architecture: [docs/design.md](/Users/twiliness/nanoclaw/docs/design.md)
- design plan: [docs/plan.md](/Users/twiliness/nanoclaw/docs/plan.md)
- plugin system design: [docs/plugin-system-design.md](/Users/twiliness/.codex/worktrees/035f/nanoclaw/docs/plugin-system-design.md)
- memory plugin design: [docs/memory-plugin-design.md](/Users/twiliness/.codex/worktrees/035f/nanoclaw/docs/memory-plugin-design.md)
- tool interface design: [docs/tool-interface-design.md](/Users/twiliness/nanoclaw/docs/tool-interface-design.md)
- tooling research: [docs/tooling-research.md](/Users/twiliness/nanoclaw/docs/tooling-research.md)

## Testing

Foundation workspace test run:

```bash
cargo test --manifest-path crates/Cargo.toml
```

App workspace test run:

```bash
cargo test --manifest-path apps/Cargo.toml
```

Targeted regressions:

```bash
cargo test --manifest-path crates/Cargo.toml -p agent --example minimal_runtime
cargo test --manifest-path crates/Cargo.toml -p mcp --test stdio_integration
cargo test --manifest-path crates/Cargo.toml -p provider --lib
```

Repository default resource caps live in
[.cargo/config.toml](/home/twiliness/nanoclaw/.cargo/config.toml).
They intentionally keep local build/test CPU usage bounded:

- Cargo build jobs default to `1`
- Rust test threads default to `1`
- Rayon worker threads default to `1`
- Tokio multi-thread worker threads default to `1`
- Bounded async tests default their Tokio blocking pool to `1`

Host apps can still override Tokio runtime limits explicitly in
`.nanoclaw/config/core.toml` with `runtime.tokio_worker_threads` and
`runtime.tokio_max_blocking_threads`. When `tokio_worker_threads` is unset,
manual host runtimes fall back to Tokio's standard `TOKIO_WORKER_THREADS`
behavior, which now lines up with the Cargo default above.

Tests that explicitly opt into the shared bounded runtime helper also inherit
`NANOCLAW_TEST_MAX_BLOCKING_THREADS`, which defaults to `1` here. Today that
includes the heavier `memory` tests plus persistence/filesystem test modules in
`store` and `tools`.

Override them explicitly when you want a faster local run:

```bash
CARGO_BUILD_JOBS=8 RUST_TEST_THREADS=8 RAYON_NUM_THREADS=8 TOKIO_WORKER_THREADS=8 NANOCLAW_TEST_MAX_BLOCKING_THREADS=8 cargo test --manifest-path crates/Cargo.toml -p memory
```

## Git Hooks

The repository ships its own git hooks under `.githooks`.

Install them once per clone:

```bash
./scripts/install-git-hooks.sh
```

Those hooks enforce two repository rules:

- `pre-commit` runs formatting for the foundation workspace (`crates/Cargo.toml`) and the app workspace (`apps/Cargo.toml`), then blocks the commit if any staged file changed and needs re-staging
- `commit-msg` requires a Conventional Commit first line such as `feat(runtime): add queue drain` or `docs: document hook installation`
