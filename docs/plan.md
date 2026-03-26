# Design Plan

## Phase 1: Bootable Runtime

Status: completed in this pass

Completed:

- Added declarative config loading from `agent-core.toml`
- Added environment override support for provider/runtime/TUI settings
- Added configured skill roots and base instructions
- Replaced the TUI `DemoBackend` path with a real `rig-core` backend
- Added real MCP client support for `stdio` and `streamable_http`
- Registered MCP tools into the same runtime tool registry as local tools
- Passed configured `hook_env` variables into command hooks
- Added root documentation and example configuration

Exit criteria:

- the TUI can start from a config file
- a real provider can answer turns
- configured MCP tools can participate in tool loops

## Phase 2: UX Hardening

Status: completed

Targets:

- stream model text into the TUI instead of waiting for whole completion responses
- show loaded provider, skills, and MCP servers in the sidebar
- surface startup failures with operator-friendly diagnostics
- add smoke tests around config-driven startup
- expose MCP prompts and resources through explicit TUI commands

Completed in this pass:

- added TUI commands for MCP server, prompt, and resource discovery
- added user-controlled prompt/resource loading into the input area
- surfaced tool behavior hints in the `/tools` listing
- added interactive approval prompts for destructive and open-world tools in the TUI
- added session-scoped allow/deny caching for repeated tool approvals
- added a startup overview sidebar plus `/status` to restore shell diagnostics
- improved startup observability for provider, store, tools, skills, and MCP connections
- added `/runs` and `/run <id-prefix>` so persisted sessions can be browsed and replayed
- streamed model text through runtime progress events into live TUI rendering
- extracted startup assembly into a reusable boot module and added config-driven smoke tests

## Phase 3: Runtime Fidelity

Status: in progress

Targets:

- preserve richer MCP prompt/resource content instead of flattening to plain text
- support provider-specific request knobs through config `additional_params`
- improve tool origin and metadata propagation across loops
- introduce a persistent run store implementation

Completed in this pass:

- tool executor failures now become error tool results and stay inside the model loop
- MCP prompt/resource payloads are preserved as structured messages and parts in the MCP layer
- OpenAI tool execution now follows the Responses API path and preserves stable `message_id` and `call_id` values
- added first-party local `web_search` and `web_fetch` tools as an optional `web-tools` feature bundle
- added basic outbound web safety controls: scheme validation, private-host blocking, domain allow/block lists
- added unit tests for HTML extraction and search-result parsing
- added a runtime-level `ToolApprovalHandler` abstraction with interactive TUI wiring
- approval denials now become error tool results instead of aborting the whole turn
- added a persistent `FileRunStore` and made the TUI prefer file-backed storage by default
- added config/env control for the run-store directory with in-memory fallback on initialization failure
- added run-store summaries so the TUI can list and replay saved runs
- added run-store search and TUI export commands for stored events and transcripts
- made provider bootstrap honor `provider.env` API keys and relaxed config parsing for partial runtime/TUI tables
- wired `provider.additional_params` through config, env override, and backend request construction
- moved dynamic hook output onto append-only transcript history instead of mutable pre-history request fields
- added append-only context compaction with automatic trigger thresholds and manual `/compact`
- compaction now preserves a recent raw-message tail instead of flattening the whole visible history into one summary
- added config/env control for context window size, compaction trigger, and retained recent message count
- redesigned the local file-tool contract so `read` returns line-numbered views with snapshot ids and slice hashes
- expanded `edit` from legacy exact replace into explicit `str_replace`, `replace_lines`, and `insert` commands with optional stale-read guards

## Phase 4: Skill System Maturity

Status: in progress

Targets:

- replace naive string matching with policy-driven activation
- support per-skill config and versioning
- expose loaded skill metadata in the TUI
- add tests for mixed skill and hook execution

Completed in this pass:

- added skill aliases to frontmatter/loading
- added `/skills [query]` and `/skill <name-or-alias>` to inspect loaded skill metadata in the TUI
- replaced prompt-string skill activation with a stable skill catalog preamble plus hook-driven behavior
- removed built-in heuristic skill matching from the runtime path so skill specialization now comes from hooks or explicit file reads

## Phase 5: Operational Readiness

Status: in progress

Targets:

- add integration tests for MCP stdio servers
- add provider-agnostic backend contract tests
- add workspace examples for OpenAI and Anthropic
- document deployment and local development workflows

Completed in this pass:

- added a real child-process integration test for MCP `stdio` servers, covering catalog, tool calls, prompts, and resources
- added provider-agnostic backend contract tests for schema coercion, rich message mapping, and tool-call event propagation
- added example `agent-core.toml` configurations for OpenAI and Anthropic workspaces
- documented targeted regression commands alongside the existing full-workspace test path

## Next Priority

The next framework gaps are standalone provider-native compaction windows where upstream APIs return opaque compacted items, plus better run-store indexing and retention beyond the current JSONL scan model.

After that, the next capability gap is better approval policy composition, richer per-skill policy/configuration on top of the new hook-driven model, and a pluggable search backend with stronger ranking and citation metadata.

Completed in this pass:

- added runtime-level approval policy composition with ordered allow/ask/deny rules
- added argument-aware approval matchers over canonical JSON pointers, plus tool-name and origin matchers
- kept shell approval handlers as the final UX boundary instead of hardcoding interactive prompts into runtime policy
- added explicit OpenAI prompt-cache request controls in the provider adapter so hosts can use `prompt_cache_key` and `prompt_cache_retention` without pushing provider JSON shape into runtime code
- added provider-managed OpenAI Responses continuation support so runtime can carry `response_id` forward and send only append-only transcript deltas after the first turn
- added OpenAI server-side compaction hints through `context_management` on the native Responses path, while keeping local runtime compaction as the provider-agnostic fallback
- upgraded `FileRunStore` to keep a mutable summary/search index sidecar next to append-only JSONL transcripts
- added run-store retention controls by run age and run count, enforced on open and append
