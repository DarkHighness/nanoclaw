# Plugin System Design

## Status

Drafted on 2026-03-26 as the implementation target for a TOML-native plugin system.

## Why This Exists

`nanoclaw` already has three real extension surfaces:

- skills via `crates/skills`
- MCP servers via `crates/mcp`
- lifecycle hooks via `runtime::hooks`

Today those surfaces are configured separately in the host shell. That is fine for a reference shell, but it does not scale once we want reusable bundles such as:

- a team skill pack
- a company MCP pack
- a memory bundle that adds tools plus its own config

The goal is to get the packaging ergonomics of Claude Code plugins and the manifest-first control plane of OpenClaw, while keeping this repository aligned with its current architecture:

- runtime stays generic
- host boot composes capabilities explicitly
- non-essential capability bundles remain feature-gated
- configuration stays TOML

## Research Baseline

Two external patterns matter here:

1. Claude Code bundles multiple extension surfaces into one installable package. The important idea is not the exact JSON schema, but the packaging shape: one plugin can contribute skills/commands, MCP definitions, hooks, and user-scoped config defaults.
2. OpenClaw separates plugin discovery and validation from runtime execution. It also treats some plugin kinds as exclusive slots, with `memory` as the clearest example.

The substrate already gives us the right low-level boundaries:

- `skills` loads packaged prompt assets
- `mcp` loads remote tool catalogs
- `runtime` executes hooks and tools
- host boot in `apps/reference-tui` assembles those pieces

That means the plugin system should be a control-plane and assembly layer, not a second runtime.

## Design Principles

### 1. TOML-only control plane

New plugin metadata and plugin configuration must be TOML.

That applies to:

- plugin manifests
- plugin-scoped MCP config
- plugin-scoped hook config
- skill metadata
- memory plugin config
- host-side plugin enablement and slot selection

This design does not introduce JSON or YAML as first-class config formats. Existing YAML frontmatter in `SKILL.md` is treated as legacy compatibility only.

### 2. Manifest-first, no hidden side effects

The host must be able to discover, validate, and explain plugin state before any plugin-backed behavior is activated.

That implies:

- plugin identity comes from a manifest file
- component paths are validated before loading
- missing or invalid plugins become diagnostics, not silent partial loads

### 3. Runtime stays plugin-agnostic

`AgentRuntime` should not learn what a plugin is.

The plugin layer resolves configuration into a deterministic activation plan:

- skill roots
- hook registrations
- MCP server configs
- optional built-in driver activations such as memory

Host boot consumes that plan and feeds the existing runtime builder.

### 4. Declarative first, code-backed only through compiled drivers

For `nanoclaw`, v1 should not attempt arbitrary third-party in-process code loading.

There are two plugin shapes:

- declarative bundle: contributes skills, hook TOML, MCP TOML, and optional instruction snippets
- driver-backed bundle: same as above, but also selects a compiled Rust driver by id for code-backed capabilities such as memory

This keeps the trust boundary legible:

- bundle content is configuration and prompt assets
- code execution remains inside the compiled binary or existing subprocess boundaries such as MCP and hook commands

### 4.1 Why not dynamic plugin code loading

Memory is the clearest example of why a pure bundle system is not enough: `memory_search`
and `memory_get` need real execution logic, index state, and provider clients.

For this repository, the correct first implementation is not `dlopen`/`libloading`,
WASM, or ad hoc scripts. It is a compiled driver registry.

Reasons:

- Rust dylib/plugin ABI is not stable enough to make local runtime plugins cheap or portable
- cross-platform loading and versioning would consume more design budget than the actual memory feature
- the repository already has a clean composition point at host boot, so compiled factories fit naturally
- security is easier to reason about when third-party code does not silently enter the process

So the rule is:

- declarative plugin bundles are discovered from TOML
- executable plugin behavior comes only from compiled driver factories registered by the host binary
- future third-party executable extensions should prefer out-of-process boundaries such as MCP, not in-process dynamic loading

### 5. Slots for exclusive capability families

Most plugin contributions are additive. Some are exclusive.

The initial exclusive slot is:

- `memory`

That matches the OpenClaw control-plane lesson and gives us a clean way to ship two memory implementations without teaching the host to special-case them ad hoc.

## Proposed Layout

### Host config

Plugin enablement lives inside `agent-core.toml`:

```toml
[plugins]
enabled = true
roots = [
  ".nanoclaw/plugins",
  "~/.config/nanoclaw/plugins",
]
include_builtin = true
allow = ["memory-core", "company-shared"]
deny = []

[plugins.slots]
memory = "memory-core"

[plugins.entries.company-shared]
enabled = true

[plugins.entries.company-shared.config]
profile = "prod"
```

Notes:

- `roots` are evaluated in order; first discovered plugin id wins
- `deny` always wins over `allow`
- slot selection is explicit and deterministic
- plugin-specific config remains nested TOML, not free-form JSON

### Plugin root

Each plugin is a directory under one configured plugin root:

```text
plugins/company-shared/
  .nanoclaw-plugin/
    plugin.toml
    hooks.toml
    mcp.toml
  skills/
    review/
      skill.toml
      SKILL.md
```

### Plugin manifest

The manifest is TOML and lives at `.nanoclaw-plugin/plugin.toml`.

Example:

```toml
id = "company-shared"
version = "0.1.0"
name = "Company Shared"
description = "Shared team workflow bundle"
kind = "bundle"
enabled_by_default = false

[components]
skill_roots = ["skills"]
hook_files = [".nanoclaw-plugin/hooks.toml"]
mcp_files = [".nanoclaw-plugin/mcp.toml"]

[[instructions]]
text = "Prefer company MCP tools when they satisfy the request."

[defaults]
profile = "dev"
```

For a driver-backed plugin:

```toml
id = "memory-core"
version = "0.1.0"
name = "Memory Core"
description = "Workspace markdown memory with local indexing"
kind = "memory"
enabled_by_default = true
driver = "builtin.memory-core"
```

## Skill Packaging Change

To satisfy the TOML-only rule, skill metadata should move out of YAML frontmatter.

### New preferred skill layout

```text
skills/review/
  skill.toml
  SKILL.md
```

`skill.toml`:

```toml
name = "review"
description = "Use for code review tasks."
aliases = ["cr"]
tags = ["review", "swe"]
```

`SKILL.md` remains the prompt body only.

### Migration rule

During transition:

1. if `skill.toml` exists, it is authoritative
2. else fall back to legacy `SKILL.md` YAML frontmatter
3. new plugins and new built-ins must ship `skill.toml`

This lets us preserve existing skills while moving the design toward TOML everywhere.

## Hook And MCP Packaging

### Hook files

Plugin hook files are plain TOML documents that deserialize into existing `HookRegistration` shapes.

Example `.nanoclaw-plugin/hooks.toml`:

```toml
[[hooks]]
name = "review-reminder"
event = "UserPromptSubmit"

[hooks.handler]
type = "prompt"
prompt = "If the task is a review, prioritize bugs, regressions, and missing tests."
```

The loader should accept one or more `[[hooks]]` arrays per file and reuse the current `types::HookRegistration` data model.

### MCP files

Plugin MCP config is also TOML and reuses `McpServerConfig`.

Example `.nanoclaw-plugin/mcp.toml`:

```toml
[[mcp_servers]]
name = "internal-docs"

[mcp_servers.transport]
transport = "stdio"
command = "uvx"
args = ["internal-docs-mcp"]
cwd = "."
```

The plugin loader is responsible for rebasing relative paths to the plugin root before handing them to `crates/mcp`.

## Resolution Model

The plugin crate should expose four distinct stages:

1. discovery
2. validation
3. enablement and slot selection
4. activation planning

### Discovery

Input:

- configured plugin roots
- builtin plugin root if enabled

Output:

- `DiscoveredPlugin` entries with manifest path, plugin root, and parse diagnostics

### Validation

Validation rules:

- plugin id must be unique within the resolved search order
- all declared component paths must stay under the plugin root
- referenced hook and MCP files must parse
- `driver` ids must resolve against the compiled driver registry
- plugins selected into exclusive slots must declare the matching `kind`

### Enablement

The effective state of a plugin is derived from:

1. global `[plugins].enabled`
2. `deny`
3. `allow`
4. `plugins.entries.<id>.enabled`
5. manifest `enabled_by_default`

State reporting should distinguish:

- enabled
- disabled
- missing
- invalid

### Activation plan

The final output is a pure data structure:

```rust
pub struct PluginActivationPlan {
    pub instructions: Vec<String>,
    pub skill_roots: Vec<PathBuf>,
    pub hooks: Vec<HookRegistration>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub driver_activations: Vec<DriverActivationRequest>,
    pub diagnostics: Vec<PluginDiagnostic>,
}
```

The important boundary is that runtime consumes the resolved results, not plugin manifests directly.

For driver-backed plugins, the activation plan carries a request to instantiate a compiled driver:

```rust
pub struct DriverActivationRequest {
    pub plugin_id: String,
    pub driver_id: String,
    pub config: toml::Table,
}
```

## Crate Shape

Add a new edge crate:

- `crates/plugins`

Responsibilities:

- parse plugin manifests and component TOML
- resolve enablement and slots
- rebase relative paths
- produce `PluginActivationPlan`
- expose diagnostics for TUI/host reporting

Non-responsibilities:

- no direct model/runtime changes
- no dynamic code loading
- no marketplace/install workflow in the first slice

## Driver Registry

Some plugins need compiled behavior. Memory is the first case.

Instead of dynamic libraries, use a small compiled registry:

```rust
pub trait PluginDriverFactory: Send + Sync {
    fn id(&self) -> &'static str;
    fn activate(&self, config: toml::Table, ctx: &PluginDriverContext)
        -> anyhow::Result<PluginDriverActivation>;
}
```

`PluginDriverActivation` should be deliberately small:

- local tools to register
- hook registrations to append
- startup diagnostics

This matches the current repository philosophy better than a second general-purpose runtime.

### Execution path for a driver-backed plugin

The intended control flow is:

1. `crates/plugins` parses TOML and resolves `driver = "builtin.memory-core"`
2. host boot looks up that id in `PluginDriverRegistry`
3. the driver factory builds its runtime objects from typed TOML config
4. the factory returns concrete registrations such as `Arc<dyn Tool>`
5. boot merges those registrations into the normal `ToolRegistry`

That means the manifest never "executes code" by itself. It selects compiled code that the host already knows how to construct.

## Host Boot Integration

This is now the active boot model in both `apps/reference-tui` and `apps/code-agent`.

Boot does:

1. load `agent-core.toml`
2. resolve plugin activation plan
3. load skills from plan skill roots
4. load hooks from plan hook files plus skill metadata
5. connect MCP servers from plan
6. activate selected drivers such as `memory`
7. register resulting local tools into `ToolRegistry`

The runtime builder API can stay unchanged.

The current codebase centralizes the shared parts of that flow in
`crates/core/src/plugin_boot.rs` so `apps/reference-tui` and `apps/code-agent`
do not drift on:

- builtin plugin-root inclusion
- discovery-to-activation-plan resolution
- builtin memory driver activation
- `embedding.api_key_env` secret materialization
- host-specific unknown-driver policy (`warn` vs `error`)

## Compatibility Strategy

The existing top-level host config is still useful. We should not break it immediately.

Compatibility rule:

- current `skill_roots` and `mcp_servers` host config are converted into an implicit synthetic plugin during boot

That gives us a migration path:

1. existing workspaces still boot
2. new plugin config can be added incrementally
3. once built-in bundles exist, old top-level fields can be deprecated

## Cargo Feature Interaction

Plugin config cannot activate code that was not compiled into the binary.

That means:

- declarative skills/hooks/MCP bundles are always loadable
- driver-backed plugins require the corresponding Cargo feature to be enabled
- if a manifest selects an unavailable driver, validation reports a clear diagnostic

This preserves the repository rule that non-essential bundles stay behind explicit features.

## Security Model

This system improves organization, not trust.

Trusted surfaces:

- plugin hook commands can execute subprocesses
- plugin MCP config can launch external servers
- plugin skills can influence model behavior

The plugin loader should therefore enforce only structural safety:

- no path escape outside plugin root
- deterministic discovery order
- explicit diagnostics for invalid content

It should not pretend that third-party plugin content is sandboxed.

## Implementation Slices

### Slice 1

- add `crates/plugins`
- add TOML manifest types and discovery
- add activation-plan tests

### Slice 2

- add `skill.toml` support to `crates/skills`
- keep YAML frontmatter as compatibility fallback

### Slice 3

- move `apps/reference-tui` boot to plugin activation plan
- add startup diagnostics for enabled/disabled/invalid plugins

### Slice 4

- add driver registry and memory slot integration
- ship builtin `memory-core` and `memory-embed` manifests

### Slice 5

- move `apps/code-agent` boot to the same plugin activation path
- persist `memory-embed` chunk embeddings for restart-safe incremental reuse

### Slice 6

- extract shared host boot helpers into the `agent` umbrella crate
- keep host-specific policy outside the helper where behavior genuinely differs

## Sources

- Claude Code plugins: <https://docs.claude.com/en/docs/claude-code/plugins>
- Claude Code plugin reference: <https://docs.claude.com/en/docs/claude-code/plugins-reference>
- OpenClaw plugins overview: <https://docs.openclaw.ai/plugins>
- OpenClaw plugin bundles: <https://docs.openclaw.ai/plugins/bundles>
- OpenClaw plugin manifest: <https://docs.openclaw.ai/plugins/manifest>
- OpenClaw plugin architecture: <https://docs.openclaw.ai/plugins/architecture>
