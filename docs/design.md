# Architecture Design

## Goals

The workspace is organized around one idea: keep the agent foundation small, composable, and provider-agnostic, then attach concrete products and integrations at the edges.

The host application is expected to compose the runtime in Rust code. File-based config, terminal UI, and other operator workflows are outer-shell concerns, not framework-defining primitives.

The design centers on four layers:

1. Model execution
2. Tool execution
3. Skill packaging and discovery
4. Outer host integration

## Workspace Layout

### Minimal Embedded Closure

The smallest practical runtime closure is:

- `types`
- `runtime`
- `tools`
- `skills`
- one provider adapter such as `provider`

Everything else is an edge layer around that closure:

- `mcp` for MCP integration
- `store` for persistence and replay
- `apps/reference-tui` for a removable reference terminal shell with its own private config module

The root foundation workspace excludes host applications so the default build and test path stays base-runtime first.

Directory naming follows that split directly:

- infrastructure crates under `crates/`
- host applications under `apps/`

The umbrella crate lives in `crates/core`, but its Rust package name is `agent` to avoid colliding with Rust's standard `core` crate.

### `types`

Shared protocol types for:

- messages
- tool calls and results
- run events
- hooks
- ids and errors

This crate is the contract surface between the rest of the workspace.

### `runtime`

Owns the turn loop:

- carries stable base instructions
- accepts append-only transcript history
- submits model requests
- receives text and tool call events
- executes tools through the shared registry
- appends transcript and run events
- evaluates hooks before and after key lifecycle events
- exposes runtime progress events to outer hosts

The runtime is intentionally unaware of any one provider, any one UI, or any one config shape.
It still exposes runtime-native coordination primitives such as append-only steering messages,
queued runtime commands, and loop detection.

### `tools`

Provides the local tool abstraction plus the core built-ins:

- `read`
- `write`
- `edit`
- `patch`
- `glob`
- `grep`
- `list`
- `bash`

Optional tool bundles are compiled behind features. Today that includes:

- `todo_read`
- `todo_write`
- `task`
- `web_search`
- `web_fetch`
- `code_symbol_search`
- `code_document_symbols`
- `code_definitions`
- `code_references`

Provider-hosted tools and MCP tools enter through the same runtime tool boundary instead of bespoke client logic.

For code-editing tools specifically, the current foundation now treats file access as a two-step contract:

- `read` exposes line-numbered views plus stable file snapshot ids and slice hashes
- `write` exposes full-file create/replace with explicit missing/existing-file policy plus optional snapshot guards
- `edit` exposes structured local mutations (`str_replace`, `replace_lines`, `insert`) with optional freshness guards
- `patch` exposes staged multi-file `write` / `edit` / `delete` operations so larger refactors do not overload `edit` or `write`

That contract is documented in detail in [tool-interface-design.md](/Users/twiliness/nanoclaw/docs/tool-interface-design.md).
Its current alignment against industrial tool contracts and the next implementation priorities are documented in [tooling-industrial-alignment.md](/Users/twiliness/nanoclaw/docs/tooling-industrial-alignment.md).
The local process boundary and sandbox model are documented separately in [sandbox-design.md](/Users/twiliness/nanoclaw/docs/sandbox-design.md).

### `skills`

Loads skills from host-provided roots. Each skill folder contains a `SKILL.md` with YAML frontmatter and optional subdirectories such as:

- `references/`
- `scripts/`
- `assets/`

Skills are treated as first-class runtime assets. They contribute:

- stable skill catalog metadata for the system preamble
- hook registrations, when a skill package provides hooks
- discoverable files that the model or host can read explicitly

The runtime no longer auto-activates skills through prompt-string heuristics.

### `provider`

Owns provider-backed model execution. It translates `types` requests into concrete provider calls and currently supports:

- OpenAI
- Anthropic

The OpenAI path is aligned with the Responses API shape. The adapter preserves stable `message_id` and `call_id` fields so provider state can be audited without leaking provider-specific naming into the rest of the runtime.
When hosts enable OpenAI response chaining, the adapter also preserves `response_id` as runtime continuation state, uses top-level Responses `instructions`, and can attach `context_management` compaction hints without forcing the runtime layer to speak provider JSON directly.

### `mcp`

Owns MCP integration. It supports:

- `stdio` servers through `rmcp::transport::TokioChildProcess`
- `streamable_http` servers through `rmcp::transport::StreamableHttpClientTransport`

It can:

- connect to servers
- list tools, prompts, and resources
- call tools
- read resources
- fetch prompts

The bridge layer converts remote MCP tool metadata into local runtime tool registrations.

### `store`

Provides run event storage and transcript replay. The workspace includes:

- `InMemoryRunStore` for tests and lightweight embeddings
- `FileRunStore` for JSONL-backed persistence plus a mutable summary/search index sidecar
- run summaries for browsing and replay
- simple run search over prompts, transcript text, tool usage, and other stored event fields
- optional retention controls by run count or run age

### `apps/reference-tui`

This crate is a reference-shell layer, not part of the minimal foundation closure.

`apps/reference-tui` currently provides:

- operator-facing tool approval UX
- startup dashboards
- run browsing, replay, and export commands
- MCP prompt/resource inspection commands
- reference shell wiring for provider boot, skill loading, and persistent storage
- a shell-local config module for file/env loading and shell-oriented defaults

Hosts embedding the foundation should treat this as an example of one application shape, not as a required framework surface.

## Runtime Flow

1. The host builds a stable system preamble in Rust code.
2. The host selects a provider adapter and constructs a `ModelBackend`.
3. The host registers the tools it wants exposed to the runtime.
4. Optional integrations such as MCP, persistence, and operator UX are attached at the edges.
5. Optional skill packages are loaded into a `SkillCatalog`.
6. The runtime starts a turn when the host submits input.
7. The runtime assembles:
   - base instructions
   - stable skill catalog metadata
   - transcript history
   - tool definitions
   - append-only hook-generated transcript entries
8. The provider backend returns text and/or tool calls.
9. Tool calls execute through the shared tool registry.
10. Before oversized requests are sent, the runtime can compact only the older visible prefix into an appended summary while keeping recent raw messages intact.
11. Tool results are appended to the transcript and the model loop continues.
12. Loop detection watches for repeated tool-call churn and no-progress patterns before the runtime drifts into pathological repetition.
13. If a tool requires approval, the runtime suspends execution and asks the host through an injected approval handler.
14. If a tool executor fails locally, approval is denied, or loop detection blocks a pathological repeated tool call, the failure is converted into an error tool result and fed back into the model loop instead of aborting the whole turn.

## Key Properties

- History is append-only from the model's point of view. Static instructions stay in the fixed preamble, while dynamic hook output is appended as transcript messages.
- Compaction is also append-only. The runtime summarizes older visible context into a new summary message while preserving a recent raw tail.
- Tool schemas are exposed from a sorted registry so the tool list order is stable across turns and process restarts when the configured tool set is unchanged.
- `message_id` and `call_id` are retained as stable audit fields even when a provider omits them.
- Skills are surfaced as catalog entries and hooks, not through client-side prompt matching.
- Runtime approval is expressed through an injected `ToolApprovalHandler`, not hardcoded to one shell.
- Runtime approval policy is also composable before any shell UX enters the picture. Hosts can attach first-match allow/ask/deny rules over tool name, tool origin, and selected argument fields, then fall through to an interactive or automatic handler only when policy leaves the request unresolved.
- The runtime does not expose a fixed global iteration cap. Code-agent behavior should rely on
  progress-aware loop detection, stop conditions, approvals, and hook decisions instead of a small
  hardcoded cycle budget.

## Concurrency And Performance Rules

- Channel ownership should reflect the execution model. When there is a single consumer, that owner should hold the receiver directly instead of placing the receiver behind a mutex.
- `tokio::sync::Mutex` and `tokio::sync::RwLock` are reserved for state that truly has to remain locked across `.await`. Plain in-memory registries, caches, and snapshots should prefer synchronous locks or task ownership.
- Long-lived I/O sessions such as shells, transports, or background workers should move toward task-plus-channel coordination instead of accumulating `Arc<Mutex<_>>` fields.
- Startup/discovery flows such as MCP connection, skill loading, and replay-heavy search should use bounded concurrency rather than accidental serial loops, while preserving stable output order when callers depend on it.
- Streaming paths should avoid cloning ever-growing buffers on every delta. Observers should accumulate deltas locally instead of forcing the runtime to clone the full assistant buffer on each token.
- Environment variable metadata and lookup rules live in `crates/env`, including descriptions for every supported key. Provider adapters and host apps should consume that shared surface instead of scattering raw env access.
- Structured `tracing` is part of the substrate contract. Hosts decide where logs are written, but runtime/provider/tool/store layers should emit enough spans and events to diagnose retries, continuation loss, approvals, tool churn, and background session behavior.

## Reference Shell Configuration

The repository still includes a declarative config layer, but that layer belongs to the removable reference shell rather than the runtime foundation.

The private config module inside `apps/reference-tui` currently supports:

- provider selection and request defaults
- runtime loop limits for the reference shell
- run-store location for the reference shell
- skill root discovery for the reference shell
- MCP server definitions for the reference shell
- hook environment variables
- optional shell-level system prompt text

Hosts embedding the foundation should define their own config layer, or none at all.

## Tooling Notes

- MCP prompt handling follows the user-controlled model from the MCP prompts spec. The reference shell exposes prompts as commands instead of treating them as model-callable tools.
- MCP resource handling follows the application-mediated model from the MCP resources spec. Resources are discoverable and loadable through the host layer, then reviewed before submission.
- Local tool annotations use MCP hint names so local and remote tools can be rendered consistently in one registry.
- Feature-enabled local web tooling follows the same two-step pattern as hosted agent stacks: `web_search` for discovery, then `web_fetch` for retrieval.
- `web_search` now goes through a provider boundary instead of hardcoding the Bing RSS bootstrap path as the contract. The request surface carries locale, freshness, and source mode explicitly, backend metadata makes fallback capability gaps visible, search results expose stable citation/source ids plus feed-provided source names, and local freshness filtering reports when it only ran in best-effort mode.
- `web_fetch` now keeps extracted block ranges and citation ids alongside the flattened text window, so pagination and source attribution can follow the structured extraction instead of reparsing transcript prose.
- `ToolExecutionContext` carries both host root policy (`workspace_root`, `worktree_root`, `additional_roots`) and per-call runtime scope (`run_id`, `session_id`, `turn_id`, `tool_name`, `tool_call_id`) so local tools can stay generic while still participating in audit and path-control flows.
- Runtime observer tool lifecycle updates now project from the same persisted run-event envelopes used by the store, so hosts can correlate live tool start/finish/failure notifications with durable history by shared event ids.
- Feature-enabled local code-intel tooling follows the same request families as LSP (`workspace/symbol`, `textDocument/documentSymbol`, `textDocument/definition`, `textDocument/references`), while keeping the backend host-pluggable instead of hardcoding one language server process contract.
- Provider streaming passes through the `ModelBackend` boundary into runtime progress events, and hosts can consume those events however they want.
- The provider adapter now exposes explicit OpenAI prompt-cache request controls (`prompt_cache_key`, `prompt_cache_retention`) instead of forcing hosts to smuggle them through opaque JSON. Those controls stay provider-scoped in the adapter layer and are omitted for non-OpenAI providers.
- The runtime now distinguishes between full visible-transcript requests and provider-managed continuation windows. When an upstream provider exposes durable response chaining, runtime can send only append-only transcript growth while still keeping its own transcript immutable and fully auditable.
- The OpenAI adapter now has a native Responses streaming path for stateful turns. That path preserves `response_id`, maps `previous_response_id` retries back into runtime continuation handling, and can emit server-side compaction hints through `context_management`.
- Startup assembly for the reference shell lives in a testable boot module, so that shell's config parsing, provider wiring, skill loading, and store fallback can be exercised without launching the full shell loop.
- MCP `stdio` support is guarded by a real child-process integration test instead of only mock-client coverage.
- The provider backend has provider-agnostic contract tests around schema coercion, message conversion, and event/origin propagation.

## Deliberate Tradeoffs

- Local runtime compaction and OpenAI server-side compaction now coexist, but only the request-hint path is integrated. The standalone OpenAI `/responses/compact` window is still not mapped into first-class runtime transcript items, because the foundation does not yet preserve opaque provider-only compaction items as replayable message objects.
- The default persistent store still uses append-only JSONL transcripts as the durable source of truth, but now pairs them with a small mutable index sidecar for summaries, search prefiltering, and retention. It still does not provide multi-process coordination or a heavier full-text index backend.
- The current approval flow now supports runtime-level rule composition in addition to shell-side prompts. Hosts can auto-allow, deny, or require review for matching tool/origin/argument patterns, but persistent allowlists and richer policy storage are still outer-host concerns.
- Feature-enabled `web_search` is intentionally lightweight today. The default fallback backend still does not provide hosted-tool quality ranking, citations, or a richer provider with true freshness/source-mode enforcement.
- Feature-enabled `web_fetch` extracts readable HTML/text content, but binary documents such as PDFs are still summarized instead of fully parsed.

## Extension Points

The next clean extension points are:

- richer run-store filtering and stronger multi-process/index backends
- richer provider request controls
- standalone provider-native compaction windows where an upstream API can preserve more structured state than a local summary string
- persistent approval policy storage and richer host-managed approval caches
- richer loop-detection policies and model-aware progress heuristics
- richer explicit skill policy and package controls
- richer MCP prompt/resource consumption in the model context
- pluggable search backends with richer citation metadata for the optional web-tools bundle
- better binary document extraction for feature-enabled `web_fetch`
