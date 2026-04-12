# Tool Protocol Alignment

Date: 2026-03-30

Status: Active (`P0` complete, `P1` complete, `P2` started)

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
- session-based interactive exec surfaces
- paged `web_fetch` and structured `web_search`
- optional code-intel tools
- child-agent tools plus revision-guarded plan state

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
- runtime `ToolRegistry` now exposes a shared-state dynamic registration path
- `DynamicToolSpec` now exists as a first-class protocol type for runtime-added
  local tools
- connected MCP servers now project their resources through dedicated
  `list_mcp_resources` and `read_mcp_resource` dynamic tools
- the old `todo_*` workflow state has been collapsed into one `update_plan`
  tool that keeps Codex-style `step` and `status` payloads while preserving the
  existing shared runtime plan state
- `ToolResult` now includes first-class `attachments` and `continuation`
- representative continuation emitters are live for:
  - `read` via file-window cursors
  - `exec_command` via session stream cursors
  - `web_fetch` via document-window cursors
- image reads now emit a first attachment record instead of hiding artifact
  identity only inside ad hoc metadata
- runtime now filters model-visible tools by typed availability instead of
  blindly forwarding every registered tool to every provider
- runtime and tool discovery now evaluate typed availability against the active
  provider, concrete model id, optional agent role, and host capability flags
  instead of stopping at a provider-only filter
- host-mediated prompt tools now advertise explicit capability flags so
  non-interactive sessions hide `request_user_input` and
  `request_permissions` instead of exposing tools that can only fail at
  runtime
- already-registered host subprocess surfaces now re-evaluate their typed
  availability from the active session permission mode instead of freezing that
  decision at boot:
  - `exec_command` and command-backed custom tools stay registered but appear
    to the model only when the host exposes the `host-process-surfaces`
    capability
  - already-connected local-process MCP tools and MCP resource surfaces share
    the same feature gate, including execution-time filtering for aggregate MCP
    resource listing and reads
- stdio MCP servers that were skipped at startup now reconnect when a later
  session permission-mode switch re-enables host subprocess surfaces, and they
  disconnect again when the session returns to a mode that cannot service local
  child processes
- OpenAI Responses mapping now supports freeform/custom tools, including
  transcript replay through `custom_tool_call` and
  `custom_tool_call_output`
- provider-specific patch surfaces are now live:
  - `apply_patch` is exposed as a freeform grammar tool for GPT-5-family
    OpenAI models
  - `patch` remains a structured function tool for Anthropic

Still pending inside this pass:

- boot-skipped local subprocess surfaces still need lazy reconnect or runtime
  reload paths if they should appear after a later permission-mode switch:
  - other startup-gated local helper surfaces such as managed subprocess-backed
    hooks or code-intel helpers

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
  `read`, `write`, `edit`, provider-specific patch surfaces (`apply_patch` on
  OpenAI, `patch` on Anthropic), `glob`, `grep`, `list`
- execution:
  `exec_command`, `write_stdin`, `js_repl`
- web:
  `web_fetch`, `web_search`
- code intelligence:
  `code_symbol_search`, `code_document_symbols`, `code_definitions`,
  `code_references`
- agentic:
  `task`, `task_batch`, `spawn_agent`, `send_input`, `wait_agent`,
  `resume_agent`, `list_agents`, `close_agent`

The child-control surfaces now follow Codex-style identifiers for
`send_input`, `wait_agent`, `resume_agent`, and `close_agent`. `spawn_agent`
now accepts the Codex-style launch fields `fork_context`, `model`, and
`reasoning_effort`, and the runtime honors them with real execution semantics.
`spawn_agent` and `send_input` now forward `message + items` as structured
user messages all the way into the child runtime instead of flattening them
into steering text. `send_input interrupt=true` now has real restart semantics
instead of degrading to a queued follow-up, and both `local_image` and
`image_url` input items now flow through the same first-class image message
parts that provider adapters use for multimodal prompts. `local_file` now means
a workspace path, while `file` accepts either a workspace path or an
`http/https` URL. Both attach first-class file parts: OpenAI consumes them
through `input_file`, while Anthropic promotes PDF attachments to native
`document` blocks and keeps other file types on the existing readable fallback
path. `mention`, `skill`, and generic `item` payloads now travel through a
dedicated typed reference part instead of being hidden inside generic
`Resource.metadata` or `Json` wrappers. Operator-visible summaries now keep
attachment placeholders in `Message::text_content()` as well, so rollback,
search, and preview flows do not silently drop attachment-only turns. MCP
prompt/resource previews and transcript exports now share the same
operator-visible message-part renderer instead of maintaining host-local
formatting forks. Session search also now splits content-oriented matching from
operator-visible previews, so structured message parts still render with their
typed markers even when the index uses `text_content()` for matching.
- state:
  `update_plan`, `request_user_input`, `request_permissions`
- discovery:
  `tool_search`, `tool_suggest`

The `code-agent` host also now exposes a Codex-style `/permissions` control
plane command that switches the session base sandbox mode between `default` and
`danger-full-access`. That control plane is now turn-boundary only: the host
rejects permission-mode changes while a turn is actively running instead of
letting them race with model execution.

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
- plugin-defined custom tool loading
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
- directory-scanned custom tools now load from `.nanoclaw/tools`, and plugin
  manifests can now contribute model-visible custom tools through the same
  command-backed contract
- plugin ids, plugin driver ids, hook names, MCP server names, and the memory
  slot selection are now typed protocol identifiers instead of loose strings,
  but provider-facing origin labels and other display-only names remain plain
  text
- the agent tool family is useful but fragmented; naming and lifecycle are not
  yet normalized to one clear task/session/close/resume model
- `request_permissions` and `/permissions` now cover the two Codex-like
  permission control planes, and the root runtime now re-evaluates host
  subprocess surfaces from the active session permission mode instead of
  freezing stdio MCP, command hooks, and managed code-intel helpers at boot.
  `/permissions` no longer mutates that host-facing capability surface during
  an active turn; it is rejected until the runtime returns to an idle boundary.
  `request_permissions` keeps its narrower turn-local role: it can widen the
  execution policy for already-visible tools later in the same turn, but it
  does not add new tools or change tool visibility mid-turn

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

Keep the current `exec_command` / `write_stdin` session model. Align it with
the Codex-style split between "start work" and "continue work" semantics by
standardizing:

- session ids
- output windows
- lifecycle states
- approval profile metadata

The approval model should treat `write_stdin` like a continuation of an already
approved execution session, not like a fresh risky action.

### Planning And Interaction

Add first-class planning and interactive clarification tools:

- `update_plan` over the existing shared plan state, not a second workflow
  storage path
- `request_user_input` over a host-owned prompt coordinator rather than a
  runtime control-plane escape hatch

These should not be treated as one-off application commands. They are part of
the runtime contract for industrial code agents.

That also means `update_plan` and similar coordination surfaces should stay out
of the normal tool-approval path. They mutate host-owned workflow state, not the
workspace or an external system.

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
  - adapt representative continuation emitters in `read`, `exec_command`, and
    `web_fetch`
  - land the first runtime dynamic registration path on top of the richer
    protocol
  - add golden tests for local, MCP, and dynamic tool spec serialization

### P1

Once the shared protocol exists, add the missing industrial extension surfaces:

- completed:
  - replace the old `todo_read` and `todo_write` pair with a single
    `update_plan` surface that projects Codex-style `step` and `status`
    payloads
  - add `request_user_input` as a host-mediated workflow tool over a shared
    backend prompt/response coordinator, with multi-question batches,
    structured answer vectors, and host-supplied `Other` note capture
  - add directory-scanned command-backed custom tools from `.nanoclaw/tools`
    on top of the existing dynamic registry path
  - add plugin-defined custom tool exports through plugin activation plans so
    workspace and plugin tools reuse one manifest/execution contract

### P2

Then add higher-variance parity work:

- freeform `apply_patch`
- image or binary-view tools if the host app needs them
- model-aware tool exposure and substitution rules

- completed:
  - add the GPT-5-family freeform `apply_patch` surface while preserving the
    structured local `patch` tool for Anthropic
  - make typed tool availability evaluate against provider, model, role, and
    host capability flags instead of only provider identity
  - reconnect startup-skipped stdio MCP, command hooks, and managed
    code-intel helpers when the session permission mode re-enables
    host-process surfaces
  - add a shared runtime host-process gate so already-running child runtimes
    can keep their immutable startup snapshot while `exec_command`,
    `write_stdin`, command-backed custom tools, and managed helper spawns
    still fail closed after the parent session revokes host-process access
  - keep active child runtimes on their launch-time command-hook snapshot;
    permission changes now revoke host subprocess execution via the shared
    execution-time gate, and newly spawned children pick up the refreshed
    hook set
  - reject `/permissions` mode switches while a turn is still running, so host
    capability changes stay at explicit turn boundaries instead of relying on
    runtime lock serialization alone
- remaining:
  - image or binary-view tools if the host app needs them

## Immediate Next Implementation Slice

The shared protocol foundation is now in place, and MCP resources now have
their own first-class tool surfaces. The live-child runtime edge around
permission-mode-driven hook updates is now closed by contract: active child
runtimes keep their launch-time hook list, revocation is enforced by the shared
execution-time host-process gate, and new children inherit the refreshed hook
snapshot.

The recommended order is:

1. only decide whether the host app still needs extra parity work such as image
   or binary-view tools

That keeps the protocol phase bounded without reopening hook-governance
complexity after the runtime gate contract has been fixed.

## Archive Trigger

Archive this note once the shared tool protocol layer, at least one dynamic tool
path, and the first planning/interaction slice have shipped. Until then, this
file should remain the live tool-alignment reference.
