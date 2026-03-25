# Architecture Design

## Goals

The workspace is organized around one idea: keep the agent substrate small, composable, and provider-agnostic, then attach concrete products and integrations at the edges.

The host application is expected to compose the runtime in Rust code. File-based config, terminal UI, and other operator workflows are outer-shell concerns, not framework-defining primitives.

The design centers on four layers:

1. Model execution
2. Tool execution
3. Skill packaging and discovery
4. Outer host integration

## Workspace Layout

### Minimal Embedded Closure

The smallest practical runtime closure is:

- `agent-core-types`
- `agent-core-runtime`
- `agent-core-tools`
- `agent-core-skills`
- one provider adapter such as `agent-core-rig`

Everything else is an edge layer around that closure:

- `agent-core-mcp` for MCP integration
- `agent-core-store` for persistence and replay
- `apps/agent-core-tui` for a removable reference terminal shell with its own private config module

The root workspace excludes the reference shell crate so the default build and test path stays substrate-first.

### `agent-core-types`

Shared protocol types for:

- messages
- tool calls and results
- run events
- hooks
- ids and errors

This crate is the contract surface between the rest of the workspace.

### `agent-core-runtime`

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

### `agent-core-tools`

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

Provider-hosted tools and MCP tools enter through the same runtime tool boundary instead of bespoke client logic.

For code-editing tools specifically, the current substrate now treats file access as a two-step contract:

- `read` exposes line-numbered views plus stable file snapshot ids and slice hashes
- `write` exposes full-file create/replace with explicit missing/existing-file policy plus optional snapshot guards
- `edit` exposes structured local mutations (`str_replace`, `replace_lines`, `insert`) with optional freshness guards
- `patch` exposes staged multi-file `write` / `edit` / `delete` operations so larger refactors do not overload `edit` or `write`

That contract is documented in detail in [tool-interface-design.md](/Users/twiliness/nanoclaw/docs/tool-interface-design.md).

### `agent-core-skills`

Loads skills from host-provided roots. Each skill folder contains a `SKILL.md` with YAML frontmatter and optional subdirectories such as:

- `references/`
- `scripts/`
- `assets/`

Skills are treated as first-class runtime assets. They contribute:

- stable skill catalog metadata for the system preamble
- hook registrations, when a skill package provides hooks
- discoverable files that the model or host can read explicitly

The runtime no longer auto-activates skills through prompt-string heuristics.

### `agent-core-rig`

Owns provider-backed model execution. It translates `agent-core-types` requests into concrete provider calls and currently supports:

- OpenAI
- Anthropic

The OpenAI path is aligned with the Responses API shape. The adapter preserves stable `message_id` and `call_id` fields so provider state can be audited without leaking provider-specific naming into the rest of the runtime.

### `agent-core-mcp`

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

### `agent-core-store`

Provides run event storage and transcript replay. The workspace includes:

- `InMemoryRunStore` for tests and lightweight embeddings
- `FileRunStore` for JSONL-backed persistence
- run summaries for browsing and replay
- simple run search over prompts, transcript text, tool usage, and other stored event fields

### `apps/agent-core-tui`

This crate is a reference-shell layer, not part of the substrate minimum closure.

`apps/agent-core-tui` currently provides:

- operator-facing tool approval UX
- startup dashboards
- run browsing, replay, and export commands
- MCP prompt/resource inspection commands
- reference shell wiring for provider boot, skill loading, and persistent storage
- a shell-local config module for file/env loading and shell-oriented defaults

Hosts embedding the substrate should treat this as an example of one application shape, not as a required framework surface.

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
- The runtime does not expose a fixed global iteration cap. Code-agent behavior should rely on
  progress-aware loop detection, stop conditions, approvals, and hook decisions instead of a small
  hardcoded cycle budget.

## Reference Shell Configuration

The repository still includes a declarative config layer, but that layer belongs to the removable reference shell rather than the runtime substrate.

The private config module inside `apps/agent-core-tui` currently supports:

- provider selection and request defaults
- runtime loop limits for the reference shell
- run-store location for the reference shell
- skill root discovery for the reference shell
- MCP server definitions for the reference shell
- hook environment variables
- optional shell-level system prompt text

Hosts embedding the substrate should define their own config layer, or none at all.

## Tooling Notes

- MCP prompt handling follows the user-controlled model from the MCP prompts spec. The reference shell exposes prompts as commands instead of treating them as model-callable tools.
- MCP resource handling follows the application-mediated model from the MCP resources spec. Resources are discoverable and loadable through the host layer, then reviewed before submission.
- Local tool annotations use MCP hint names so local and remote tools can be rendered consistently in one registry.
- Feature-enabled local web tooling follows the same two-step pattern as hosted agent stacks: `web_search` for discovery, then `web_fetch` for retrieval.
- Provider streaming passes through the `ModelBackend` boundary into runtime progress events, and hosts can consume those events however they want.
- Startup assembly for the reference shell lives in a testable boot module, so that shell's config parsing, provider wiring, skill loading, and store fallback can be exercised without launching the full shell loop.
- MCP `stdio` support is guarded by a real child-process integration test instead of only mock-client coverage.
- The `rig` backend has provider-agnostic contract tests around schema coercion, message conversion, and event/origin propagation.

## Deliberate Tradeoffs

- The current compaction path is runtime-local and model-generated. It preserves append-only history and recent-message tails, but it does not yet integrate provider-native compaction APIs such as OpenAI Responses `/compact`.
- The default persistent store is intentionally simple JSONL. It supports browsing, replay, search, and export, but it still does not provide indexed search, retention controls, or multi-process coordination.
- The current approval flow is intentionally simple. It supports session-scoped allow/deny caching by tool identity, but it does not yet support argument-aware policies or persistent allowlists.
- Feature-enabled `web_search` is intentionally lightweight today. It does not yet provide hosted-tool quality ranking, citations, or user-location controls.
- Feature-enabled `web_fetch` extracts readable HTML/text content, but binary documents such as PDFs are still summarized instead of fully parsed.

## Extension Points

The next clean extension points are:

- richer run-store filtering, indexing, and retention
- richer provider request controls
- provider-native compaction support where an upstream API can preserve more structured state
- richer permission policy and approval caching
- richer loop-detection policies and model-aware progress heuristics
- richer explicit skill policy and package controls
- richer MCP prompt/resource consumption in the model context
- pluggable search backends with richer citation metadata for the optional web-tools bundle
- better binary document extraction for feature-enabled `web_fetch`
