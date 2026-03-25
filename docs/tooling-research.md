# Tooling Research Notes

This note captures the external references used for the current tooling pass on 2026-03-25.

## Primary Sources

- MCP tools spec (2025-06-18): [modelcontextprotocol.io/specification/2025-06-18/server/tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)
- MCP prompts spec (2025-06-18): [modelcontextprotocol.io/specification/2025-06-18/server/prompts](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts)
- Anthropic tool use guide: [docs.anthropic.com/en/docs/build-with-claude/tool-use](https://docs.anthropic.com/en/docs/build-with-claude/tool-use)
- Anthropic Agent SDK overview: [docs.anthropic.com/en/docs/claude-code/sdk](https://docs.anthropic.com/en/docs/claude-code/sdk)
- Anthropic web search tool: [docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-search-tool](https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-search-tool)
- Anthropic web fetch tool: [docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-fetch-tool](https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-fetch-tool)
- OpenAI tools overview: [platform.openai.com/docs/guides/tools?api-mode=responses](https://platform.openai.com/docs/guides/tools?api-mode=responses)
- OpenAI prompt caching guide: [platform.openai.com/docs/guides/prompt-caching](https://platform.openai.com/docs/guides/prompt-caching)
- OpenAI Responses compact API: [platform.openai.com/docs/api-reference/responses/compact](https://platform.openai.com/docs/api-reference/responses/compact)
- OpenAI Codex agent loop writeup: [openai.com/index/unrolling-the-codex-agent-loop](https://openai.com/index/unrolling-the-codex-agent-loop/)
- OpenClaw compaction concept: [docs.openclaw.ai/concepts/compaction](https://docs.openclaw.ai/concepts/compaction)
- `rig-core` OpenAI completion model docs: [docs.rs/rig-core/latest/rig/providers/openai/completion/struct.CompletionModel.html](https://docs.rs/rig-core/latest/rig/providers/openai/completion/struct.CompletionModel.html)

## Decisions Taken

### 1. Tool failures should stay inside the tool loop

Reason:

- MCP distinguishes protocol errors from tool execution errors and expects execution failures to be reported as tool results with `isError: true`.
- Anthropic's tool-use flow also assumes the client returns `tool_result` content back to the model so the model can recover, explain, or ask follow-up questions.

Implementation impact:

- `agent-core-runtime` now converts tool executor failures into `ToolResult::error(...)` transcript messages and continues the loop.

### 2. MCP prompts should be user-controlled, not silently auto-invoked by the model

Reason:

- The MCP prompts spec explicitly describes prompts as user-controlled and calls out slash commands as a natural UI pattern.

Implementation impact:

- `agent-core-tui` now exposes prompt discovery and loading with `/prompts` and `/prompt <server> <name>`.
- Prompt payloads are loaded into the input box for explicit user review and submission.

### 3. MCP resources should be application-mediated

Reason:

- The MCP resources spec describes resources as application-driven context, not the same thing as model-controlled tools.

Implementation impact:

- Resources are listed separately from tools.
- `/resource <server> <uri>` fetches the resource and loads a readable preview into the input box.

### 4. Local built-in tools should look more like a real coding agent toolset

Reason:

- Anthropic's Agent SDK documents a pragmatic baseline set for coding agents: `Read`, `Write`, `Edit`, `Bash`, `Glob`, `Grep`, plus web tools.

Implementation impact:

- The local tool registry now includes `edit`, `glob`, and `bash` alongside `read`, `write`, and `grep`.

### 5. Tool behavior hints should follow MCP vocabulary

Reason:

- MCP defines `readOnlyHint`, `destructiveHint`, `idempotentHint`, and `openWorldHint` as the standard hint vocabulary for tool behavior.

Implementation impact:

- Local tools now publish MCP-style behavior hints in their annotations.
- TUI tool listings surface the main safety-relevant hints.

### 6. OpenAI state should follow the Responses API object model

Reason:

- OpenAI's current agent/tooling surface is centered on the Responses API family.
- Responses items carry stable ids such as `message_id` and `call_id`, which are useful for auditability, compaction, and provider-native round-tripping.

Implementation impact:

- The OpenAI backend path now uses the Responses-compatible `rig` path.
- `agent-core-types` preserves `message_id` on messages and `call_id` on tool calls/results.
- Provider-specific legacy names remain only as deserialization aliases for compatibility.

### 7. Local web tooling should preserve the hosted-tool split between discovery and retrieval

Reason:

- Anthropic documents `web_search` and `web_fetch` as separate tools with distinct safety and budgeting concerns.
- OpenAI exposes web search as a first-class built-in tool in the Responses API, reinforcing that web access belongs in the tool layer rather than hidden prompt logic.
- Hosted tool stacks expose domain and result controls. A local implementation should at least preserve the same separation of concerns and basic network safety shape.

Implementation impact:

- `agent-core-tools` now provides first-party `web_search` and `web_fetch` as an optional feature bundle instead of part of the mandatory default tool set.
- `web_fetch` accepts only `http`/`https`, blocks local or private hosts by default, and extracts readable text from HTML or other text-like responses.
- `web_search` is intentionally lightweight and backend-pluggable. The default bootstrap path uses an RSS search endpoint for discovery and can be overridden with `AGENT_CORE_WEB_SEARCH_ENDPOINT`.
- Domain allow/block lists are available through environment variables so operators can narrow outbound access before a richer approval UX lands.

### 8. Approval should live at the runtime boundary, not inside individual tools

Reason:

- MCP and hosted agent systems both assume tool invocation remains client-mediated, especially for sensitive operations.
- The current project already normalizes local, MCP, and provider tools into one runtime registry. Approval belongs at that shared execution boundary.
- Per-tool ad hoc prompts would duplicate policy logic and still miss remote MCP tools.

Implementation impact:

- `agent-core-runtime` now exposes `ToolApprovalHandler`.
- `agent-core-tui` supplies the current interactive implementation and pauses on destructive or open-world tools, plus hook-driven `ask` requests.
- Approval denials are reported back as error tool results so the model can recover instead of losing the whole turn.
- The TUI approval layer now caches allow/deny decisions for the current session by tool identity, which is the smallest practical step toward reusable approval policy without entangling runtime state with UI persistence.

### 9. Generic agent runtimes should avoid client-side heuristic skill activation

Reason:

- OpenAI's Codex agent-loop writeup describes skills as loaded context assets that become available to the model, while client-side logic focuses on stable instructions, tool schemas, and append-only interaction loops.
- OpenClaw's context model similarly treats context and pruning as explicit runtime mechanics, not prompt keyword routing.
- Prompt-string matching inside the client is a brittle local policy that does not generalize across providers or future tools.

Implementation impact:

- `nanoclaw` no longer auto-activates skills based on prompt keywords, aliases, or tags.
- Loaded skills are now exposed as a stable manifest in the static preamble, and dynamic behavior is delegated to hooks or explicit file reads.

### 10. Prefix-cache-friendly runtimes should keep history append-only

Reason:

- OpenAI's prompt caching guidance depends on exact prefix reuse, and the Codex agent-loop writeup explicitly calls out stable tool descriptions and append-only history as the path to cache efficiency.
- OpenClaw documents explicit context management and pruning rather than mutating the front of the prompt on every turn.
- Reinjecting fresh hook context into `instructions` or other pre-history slots destroys cache locality because it changes tokens before the existing transcript.
- Tool definitions also need deterministic ordering. Even semantically identical tool lists can miss prefix caches if their serialized order drifts between turns.

Implementation impact:

- Dynamic hook output is now appended to transcript history as system messages.
- The static instruction preamble stays fixed, while user, assistant, tool, and hook-produced context only append at the end of model-visible history.
- Request-local `additional_context` is no longer used for normal hook output in the main runtime loop.
- Tool registration already uses a sorted map, and the workspace now has a regression test that locks the outward tool ordering to a stable lexicographic sequence.

### 11. Compaction should summarize only the older prefix and keep a recent raw tail

Reason:

- OpenClaw's compaction model preserves a summary entry plus the recent messages that still matter for short-term continuity.
- OpenAI's Responses compact API similarly treats compaction as a context-management operation over prior items, not a license to mutate the whole request frontmatter each turn.
- Flattening the entire visible transcript into one summary discards useful recent structure and weakens cache reuse because every compacted turn starts from a fresh synthetic baseline.

Implementation impact:

- `nanoclaw` now compacts only the older visible prefix and keeps a configurable number of recent raw messages intact.
- The stored transcript remains append-only. Compaction appends a new system summary and changes only the logical request view used for future model calls.
- Auto-compaction and manual `/compact` now share the same runtime path, so hooks and run events observe both flows consistently.

## Remaining Gaps

- MCP prompt arguments are discovered and displayed, but the TUI does not yet provide an argument-entry UX beyond the current basic command surface.
- Tool approvals are still coarse. Session caching exists now, but there is still no argument-aware policy, persistent allowlist, or config-driven approval rules.
- The optional local `web_search` path is a bootstrap implementation. It does not yet match hosted-tool quality for ranking, citation richness, or location-aware search controls.
- Optional `web_fetch` does not yet parse binary documents like PDFs into model-friendly text.
- The new skill model removes heuristic activation, but richer explicit skill policy and versioning still need to be designed.
