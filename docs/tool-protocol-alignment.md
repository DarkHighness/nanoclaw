# Tool Protocol Alignment

Date: 2026-03-30

Status: Active (`P0` partially implemented)

This note is the live entry point for the current tool-surface alignment pass
against OpenAI Codex and OpenCode. It converts the earlier review into a
versioned implementation target so future work can land against one explicit
contract and then be archived once the substrate catches up.

Historical background remains available in:

- `docs/archive/2026-03-26/tool-interface-design.md`
- `docs/archive/2026-03-26/tooling-research.md`

## Why This Pass Exists

`nanoclaw` already has a credible coding-agent baseline:

- line-numbered `read` windows with snapshot and selection-hash anchors
- staged multi-file `patch`
- session-based `bash`
- paged `web_fetch` and structured `web_search`
- optional code-intel tools
- child-agent tools plus revision-guarded todo state

That baseline is good enough for local coding loops, but it still falls short
of the tool protocol expected by industrial code agents in three ways:

1. the tool spec is still almost entirely "function tool" shaped
2. extension and discovery surfaces are incomplete
3. tool governance metadata is still too implicit and too weakly typed

This pass focuses on those gaps rather than reopening the already-shipped file
tool grounding work.

## Scope

This comparison includes all model-visible or runtime-loadable tool surfaces:

- built-in local tools
- dynamic tool injection
- MCP tool bridging
- MCP resources when they are exposed as model-callable surfaces
- plugin or custom local tool loading

This pass explicitly does **not** count:

- internal host APIs that are not model-visible tools
- OpenCode source files that exist on disk but are not currently registered in
  the live tool registry
- UI-only commands that never become tool calls

## Implementation Progress

The shared protocol work has already started landing in the substrate.

Completed so far:

- `ToolSpec` now carries typed `kind`, `source`, aliases, parallel-call support,
  typed availability, and typed approval metadata
- provider and MCP mapping layers preserve the richer `ToolSpec` contract
- `ToolResult` now includes first-class `attachments` and `continuation`
- representative continuation emitters are live for:
  - `read` via file-window cursors
  - `bash` via session stream cursors
  - `web_fetch` via document-window cursors
- image reads now emit a first attachment record instead of hiding artifact
  identity only inside ad hoc metadata

Still pending inside this pass:

- dynamic tool registration
- MCP resource tools
- plugin or directory-scan custom tools
- planning and user-interaction tools such as `update_plan` and
  `request_user_input`
- a freeform grammar `apply_patch` surface

## Baseline Rules

### Codex

Use the current Rust tool assembly as the baseline for model-visible tools:

- `codex-rs/tools/src/tool_spec.rs`
- `codex-rs/tools/src/responses_api.rs`
- `codex-rs/core/src/tools/spec.rs`
- `codex-rs/app-server-protocol/src/protocol/v2.rs`

Important constraints:

- Codex has a multi-kind `ToolSpec`, not just function tools
- Codex supports freeform grammar tools
- Codex marks per-tool parallel call support
- Codex promotes dynamic tools to a protocol-level concept
- Codex exposes MCP resource listing/reading as first-class tools
- Codex does **not** expose the same model-visible local file tools that
  `nanoclaw` and OpenCode do; some file operations are instead available
  through app-server surfaces outside the normal tool registry

### OpenCode

Use the current TypeScript registry and permission/runtime layers as the
baseline:

- `packages/opencode/src/tool/registry.ts`
- `packages/opencode/src/tool/tool.ts`
- `packages/opencode/src/permission/index.ts`
- `packages/opencode/src/config/config.ts`

Important constraints:

- only registered tools count as current capability
- custom tools loaded from `{tool,tools}/*.{js,ts}` count as part of the
  industrial baseline
- plugin-supplied tools count as part of the industrial baseline
- OpenCode remains function-like at the model boundary, but its runtime result,
  approval, and plugin contracts are materially richer than a plain schema plus
  text response

### nanoclaw

Use the current runtime bootstrap and type definitions as the source of truth:

- `apps/code-agent/src/backend/boot_runtime.rs`
- `crates/types/src/tool.rs`
- `crates/tools/src/registry.rs`

## Current Tool Families

### Codex

Codex currently exposes the following major tool families:

- terminal and editing:
  `exec_command`, `write_stdin`, `apply_patch`, shell variants, `js_repl`
- planning and interaction:
  `update_plan`, `request_user_input`, `request_permissions`
- discovery and resources:
  `tool_search`, `tool_suggest`, `list_mcp_resources`,
  `list_mcp_resource_templates`, `read_mcp_resource`
- media and retrieval:
  `web_search`, `view_image`, `image_generation`
- multi-agent orchestration:
  `spawn_agent`, `send_input` or `send_message`, `wait_agent`, `resume_agent`,
  `list_agents`, `close_agent`, plus batch job helpers
- dynamic and MCP-injected tools:
  dynamic tool specs and MCP tools are converted into the same outward
  tool-definition family

### OpenCode

OpenCode currently registers these major built-in families:

- file and code:
  `read`, `glob`, `grep`, `edit`, `write`, `apply_patch`
- execution:
  `bash`
- network and retrieval:
  `webfetch`, `websearch`, `codesearch`
- workflow:
  `task`, `todowrite`, `skill`, `question`
- experimental or conditional:
  `lsp`, `batch`, `plan_exit`
- plugin and custom tools:
  runtime-loaded tool modules from config directories and plugin registries

The codebase also contains `list`, `multiedit`, and `plan_enter`, but those do
not count here because they are not currently in the live registry.

### nanoclaw

`nanoclaw` currently exposes these major families from the code-agent runtime:

- file and code:
  `read`, `write`, `edit`, `patch`, `glob`, `grep`, `list`
- execution:
  `bash`, `js_repl`
- web:
  `web_fetch`, `web_search`, `web_search_backends`
- code intelligence:
  `code_symbol_search`, `code_document_symbols`, `code_definitions`,
  `code_references`
- agentic:
  `task`, `task_batch`, `agent_spawn`, `agent_send`, `agent_wait`,
  `agent_list`, `agent_cancel`
- state:
  `todo_read`, `todo_write`

This is already stronger than Codex on model-visible local file tooling, but it
is still weaker than Codex and OpenCode on protocol shape and extension
surfaces.

## Alignment Summary

### Already Strong Or Near Parity

These areas should be preserved, not redesigned from scratch:

- line-numbered file reads with freshness anchors
- staged multi-file local patching
- resumable command sessions
- structured web search results with backend metadata
- code-intel as an optional bundle instead of a mandatory default
- child-agent tooling already split from the runtime control plane

### Missing In `nanoclaw`

These are the highest-signal missing capabilities today:

- a first-class multi-kind tool spec with explicit freeform and native tool
  variants
- a grammar-based `apply_patch`-style freeform tool surface
- `update_plan`
- `request_user_input`
- a model-visible approval or permission-request tool contract
- `tool_search` and `tool_suggest` style deferred tool discovery
- dynamic tool registration as a protocol-level runtime feature
- plugin or config-directory custom tool loading
- MCP resources exposed as dedicated resource tools rather than only
  host-mediated commands
- image or binary-view tool surfaces comparable to `view_image`

### Implemented But Still Protocol-Weaker

These areas exist in `nanoclaw`, but their contracts are still behind the
industrial baseline:

- `ToolSpec` is still function-shaped; it lacks `kind`,
  `supports_parallel_tool_calls`, aliases, typed availability controls, and a
  stable source descriptor beyond the current coarse origin enum
- approval and capability hints are stored in generic annotations instead of a
  typed approval profile
- `ToolResult` now has first-class attachments and a standard continuation
  envelope, but only part of the tool surface emits typed continuations so far
- result metadata is still loosely typed and varies by tool family
- tool loading is mostly bootstrap-time and static; there is no Codex-style
  dynamic tool spec path and no OpenCode-style custom tool scan
- the agent tool family is useful but fragmented; naming and lifecycle are not
  yet normalized to one clear task/session/close/resume model
- tool exposure is not yet model-aware in the OpenCode sense, where one model
  family may see `apply_patch` while another sees `edit` and `write`

## Target Protocol

The target is not "copy Codex" or "copy OpenCode." The target is the smallest
coherent protocol that can host:

- `nanoclaw` local coding tools
- Codex-style freeform and native tools
- OpenCode-style plugin and custom tools
- MCP tools and MCP resources

### `ToolSpec`

`ToolSpec` should become a tagged protocol object instead of an always-function
object with optional extras.

```rust
pub enum ToolKind {
    Function,
    Freeform(FreeformToolFormat),
    Native(NativeToolKind),
}

pub enum NativeToolKind {
    WebSearch,
    ImageGeneration,
    ToolSearch,
    Resource,
    HostControl,
}

pub struct ToolAvailability {
    pub feature_flags: Vec<String>,
    pub provider_allowlist: Vec<String>,
    pub role_allowlist: Vec<String>,
    pub hidden_from_model: bool,
}

pub struct ToolApprovalProfile {
    pub read_only: bool,
    pub mutates_state: bool,
    pub idempotent: Option<bool>,
    pub open_world: bool,
    pub needs_network: bool,
    pub needs_host_escape: bool,
    pub approval_message: Option<String>,
}

pub struct ToolSpec {
    pub name: ToolName,
    pub description: String,
    pub kind: ToolKind,
    pub input_schema: Option<Value>,
    pub output_mode: ToolOutputMode,
    pub output_schema: Option<Value>,
    pub supports_parallel_tool_calls: bool,
    pub aliases: Vec<ToolName>,
    pub origin: ToolOrigin,
    pub source: Option<Value>,
    pub availability: ToolAvailability,
    pub approval: ToolApprovalProfile,
}
```

Key design rules:

- `input_schema` is required for function tools and optional for freeform or
  native tools
- freeform tools must declare their syntax and grammar shape
- native tools exist for provider-style or host-style built-ins that are not
  naturally modeled as function calling
- availability stays in the registry protocol rather than being open-coded in
  bootstrap branches
- approval semantics become typed instead of hiding behind ad hoc annotations

### Dynamic Tool Protocol

Dynamic tools should follow a small, stable shape close to Codex:

```rust
pub struct DynamicToolSpec {
    pub name: ToolName,
    pub description: String,
    pub input_schema: Value,
    pub defer_loading: bool,
}
```

This is intentionally narrow. The first dynamic tool protocol should solve only
runtime registration and schema exposure, not plugin installation policy.

### Resource Tool Protocol

MCP resources should not be smuggled through `read` or a host-only side panel.
They should become a distinct tool family with explicit listing and read
surfaces:

- `list_mcp_resources`
- `list_mcp_resource_templates`
- `read_mcp_resource`

The important design decision is classification, not the exact names. Resource
tools should be typed as resource-native surfaces so the runtime can keep
approval, pagination, and rendering policy separate from local filesystem tools.

### `ToolResult`

`nanoclaw` already has the right core idea: text parts plus optional structured
content. The target protocol should preserve that shape and add the missing
industrial pieces.

```rust
pub enum ToolContinuation {
    FileWindow {
        snapshot: String,
        selection_hash: Option<String>,
        next_start_line: Option<u64>,
    },
    StreamWindow {
        session_id: String,
        stdout_start_char: Option<u64>,
        stderr_start_char: Option<u64>,
    },
    DocumentWindow {
        document_id: String,
        next_start_index: Option<u64>,
    },
    TaskHandle {
        task_id: String,
        status: String,
        resumable: bool,
    },
}

pub struct ToolAttachment {
    pub kind: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub metadata: Option<Value>,
}

pub struct ToolResult {
    pub id: ToolCallId,
    pub call_id: CallId,
    pub tool_name: ToolName,
    pub parts: Vec<MessagePart>,
    pub structured_content: Option<Value>,
    pub attachments: Vec<ToolAttachment>,
    pub continuation: Option<ToolContinuation>,
    pub metadata: Option<Value>,
    pub is_error: bool,
}
```

Key design rules:

- all partial or resumable tools should emit one normalized continuation object
- attachments should be first-class rather than hidden in ad hoc metadata
- structured payloads should survive provider round-trips whenever the provider
  supports them
- when the provider does not support them, the runtime should use one canonical
  text envelope instead of tool-specific prose conventions

## Tool Family Targets

### File Tools

Keep the current `read`, `edit`, `write`, `patch`, `glob`, `grep`, and `list`
family. The main remaining work is protocol hardening:

- keep line windows and snapshot guards
- standardize continuation metadata
- make diff previews and failure diagnostics predictable across all mutators
- add a freeform `apply_patch` surface without deleting the structured local
  `patch` tool

The target is a two-tier edit surface:

- structured local edit tools for deterministic small changes
- freeform patch grammar for model families that are strongly optimized for it

### Execution Tools

Keep the current `bash` session model. Align it with the Codex-style split
between "start work" and "continue work" semantics by standardizing:

- session ids
- output windows
- lifecycle states
- approval profile metadata

### Planning And Interaction

Add first-class planning and interactive clarification tools:

- `update_plan`
- `request_user_input`

These should not be treated as one-off application commands. They are part of
the runtime contract for industrial code agents.

### Agent Tools

The current `task` and `agent_*` family is already useful, but it should be
normalized around explicit handles and lifecycle symmetry:

- spawn
- send or assign
- wait
- list
- resume when needed
- close or cancel with clear semantics

This does not require deleting the existing tools immediately. It does require a
clear migration target so the family stops growing sideways.

### Extension Surfaces

Two extension paths are required:

1. runtime dynamic registration
2. plugin or directory-scan loading

The first aligns with Codex dynamic tools and MCP tool bridging.

The second aligns with OpenCode custom tools and plugin-defined tools.

Both paths should resolve into one registry representation once loaded.

## Priorities

### P0

The first implementation slice should change protocol shape, not add a large
new bundle of tools:

- completed:
  - extend `crates/types/src/tool.rs` with `ToolKind`,
    `supports_parallel_tool_calls`, typed availability, and typed approval
    metadata
  - extend `ToolResult` with attachments and continuation
  - preserve current tool behavior while upgrading the shared contract
  - adapt representative continuation emitters in `read`, `bash`, and
    `web_fetch`
- still open:
  - teach `crates/tools/src/registry.rs` to register richer dynamic specs
  - add golden tests for local, MCP, and dynamic tool spec serialization

### P1

Once the shared protocol exists, add the missing industrial extension surfaces:

- dynamic tool registration
- MCP resource tools
- plugin or custom tool loading
- `update_plan`
- `request_user_input`

### P2

Then add higher-variance parity work:

- freeform `apply_patch`
- tool discovery surfaces such as `tool_search` and `tool_suggest`
- image or binary-view tools if the host app needs them
- model-aware tool exposure and substitution rules

## Immediate Next Implementation Slice

The shared protocol foundation is now in place. The next slice should stop
expanding local tool contracts and instead close the biggest capability gaps.

The recommended order is:

1. add runtime dynamic tool registration on top of the richer `ToolSpec`
2. expose MCP resources as dedicated resource tools
3. add the first planning and interaction tools:
   `update_plan` and `request_user_input`
4. only then decide whether freeform `apply_patch` or plugin/directory-scan
   loading should land first

That keeps the protocol phase bounded and moves the project toward the missing
industrial surfaces instead of polishing the already-upgraded local results
indefinitely.

## Archive Trigger

Archive this note once the shared tool protocol layer, at least one dynamic tool
path, and the first planning/interaction slice have shipped. Until then, this
file should remain the live tool-alignment reference.
