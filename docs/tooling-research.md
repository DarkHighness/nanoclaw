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
- OpenAI background mode guide: [platform.openai.com/docs/guides/background](https://platform.openai.com/docs/guides/background)
- OpenAI prompt caching guide: [platform.openai.com/docs/guides/prompt-caching](https://platform.openai.com/docs/guides/prompt-caching)
- OpenAI Responses compact API: [platform.openai.com/docs/api-reference/responses/compact](https://platform.openai.com/docs/api-reference/responses/compact)
- MCP progress utility: [modelcontextprotocol.io/specification/draft/basic/utilities/progress](https://modelcontextprotocol.io/specification/draft/basic/utilities/progress)
- OpenAI Codex agent loop writeup: [openai.com/index/unrolling-the-codex-agent-loop](https://openai.com/index/unrolling-the-codex-agent-loop/)
- OpenClaw compaction concept: [docs.openclaw.ai/concepts/compaction](https://docs.openclaw.ai/concepts/compaction)
- `rig-core` OpenAI completion model docs: [docs.rs/rig-core/latest/rig/providers/openai/completion/struct.CompletionModel.html](https://docs.rs/rig-core/latest/rig/providers/openai/completion/struct.CompletionModel.html)
- Language Server Protocol 3.17 spec: [microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/)
- SCIP protocol and toolkit: [github.com/sourcegraph/scip](https://github.com/sourcegraph/scip)
- Universal Ctags reference manual: [docs.ctags.io/en/latest/man/ctags.1.html](https://docs.ctags.io/en/latest/man/ctags.1.html)
- LocAgent: [arXiv:2503.09089](https://arxiv.org/abs/2503.09089)

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
- Provider adapters now emit the substrate's canonical field names directly instead of preserving compatibility aliases in the core types.

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

### 12. File tools should share a single grounding model

Reason:

- Anthropic's text editor tool uses line-oriented `view` output and pairs it with explicit edit commands such as `str_replace` and `insert`, which keeps reads and writes in the same conceptual frame.
- OpenAI's `apply_patch` guidance suggests that larger edits should move toward structured diff application instead of ever-larger raw string replacements.
- OpenCode keeps `read`, `edit`, and `patch` distinct, reinforcing the value of separating discovery, local precise edits, and larger diff application.
- Trace-Free+, Tool-Genesis, OpaqueToolsBench, and ToolComp all point in the same direction: interface quality, parameter clarity, and failure observability materially affect agent quality, and small interface flaws amplify downstream.
- I did not find a primary source describing Cursor's exact line-hash contract, so per-line consistency hashes should be treated as an inferred product pattern rather than a sourced baseline.

Implementation impact:

- `read` now defaults to line-numbered output and surfaces a stable file snapshot id plus a slice hash for the visible range.
- `write` now exposes create/overwrite policy plus optional `expected_snapshot` guards, instead of silently acting as an unconditional overwrite escape hatch.
- `edit` now supports explicit `str_replace`, `replace_lines`, and `insert` commands, with optional `expected_snapshot` and `expected_selection_hash` guards.
- `patch` now stages multi-file `write` / `edit` / `delete` operations in memory first, so a failed later operation does not partially commit earlier mutations.
- `grep`, `glob`, and `list` now return stable text headers plus structured metadata arrays, so discovery tools line up better with the read/edit path.
- `bash`, `web_fetch`, `web_search`, `todo_write`, and `task` now expose more structured inputs and metadata rather than relying on one-off text blobs alone.
- The substrate now accepts only the canonical tool fields (`start_line` / `line_count`, explicit `operation`, explicit `prompt` / `agent`) instead of carrying compatibility aliases in development builds.
- The design choice for now is file-level and slice-level hashes, not per-line hashes, because they preserve stale-read detection without overwhelming the prompt with checksum noise.

### 13. Optional code-intel should follow LSP request families but remain backend-pluggable

Reason:

- LSP already standardizes the navigation request families we want (`workspace/symbol`, `textDocument/documentSymbol`, `textDocument/definition`, `textDocument/references`), so local tool names should map cleanly to those categories instead of inventing one-off semantics.
- SCIP and ctags are both real-world index formats/tools used in production stacks; both reinforce separating the model-facing navigation contract from the underlying index/provider implementation.
- LocAgent-style SWE localization results continue to support explicit entity/symbol localization as a first-order step before patching.

Implementation impact:

- `agent-core-tools` now exposes a feature-gated `code-intel` bundle with four local tools: `code_symbol_search`, `code_document_symbols`, `code_definitions`, and `code_references`.
- The feature provides a `CodeIntelBackend` trait so hosts can plug in an LSP client, an external index, or another backend without changing the tool contract.
- The default concrete backend is `WorkspaceTextCodeIntelBackend`, a lexical in-workspace indexer that proves the contract and keeps tests deterministic when no external code-intel daemon is available.
- `code_references` uses the substrate's canonical `include_declaration` field directly, even when the underlying backend is LSP-shaped.

### 14. Tool contracts should carry async handles and normalized identities where execution is multi-hop

Reason:

- OpenAI's background mode is explicitly handle-based (`response_id` plus follow-up retrieval/cancel), so long-running work should not require replaying the full request body on every follow-up.
- MCP utility guidance exposes explicit progress/correlation semantics across calls. Even when local tools are not full MCP servers, this reinforces returning stable ids and status snapshots for long-running tasks.
- Runtime transcripts and compaction rely on deterministic tool call/result identity. Adapter boundaries should preserve upstream ids, but normalize the runtime-facing call id.

Implementation impact:

- `bash` now supports `start` / `poll` / `cancel` with `session_id`, explicit state metadata, and output window offsets for continuation.
- `web_fetch` now emits `document_id` and accepts `expected_document_id` to guard continuation calls against stale or changed responses.
- `web_search` now emits stable per-result ids and supports `offset` pagination so citation/retrieval loops can continue deterministically.
- `task` now normalizes `status`, `summary`, and `artifacts` metadata without removing raw text output.
- `mcp_adapter` now enforces local call/tool identity while preserving upstream ids inside metadata for audit and correlation.

### 15. Tool execution context should carry host roots plus runtime call scope

Reason:

- OpenAI's current tooling stack emphasizes stable `call_id` / `message_id` style identities because downstream auditing, compaction, and recovery depend on correlating execution results back to the loop that produced them.
- MCP keeps tool invocation application-mediated, which means host-level root policy and execution correlation belong at the runtime boundary rather than hidden inside individual tool implementations.
- Generic code agents often need more than one allowed local root: a checked-out worktree, a sandbox root, and a small set of explicitly mounted auxiliary roots. Re-encoding that policy separately inside every file-like tool does not scale.

Implementation impact:

- `ToolExecutionContext` now carries `worktree_root`, `additional_roots`, and runtime ids (`run_id`, `session_id`, `turn_id`, `tool_name`, `tool_call_id`).
- `agent-core-runtime` now clones a per-call scoped context before tool execution so each local tool sees the exact runtime identifiers for the current invocation.
- Path-policy helpers now support validating a candidate path against multiple allowed roots, and local fs/process/code-intel tools delegate root checks through that shared context method instead of open-coding one-root assumptions.
- The minimal runtime example and reference shells now initialize `worktree_root` explicitly so hosts can opt into multi-root policy without inventing a parallel config surface.

### 16. Approval policy should be a runtime substrate concern, not just a shell prompt

Reason:

- OpenAI's MCP guidance explicitly separates `require_approval` from the transport/tool definition, and also supports narrower "never approve these tools" allowlists instead of forcing one global mode.
- Anthropic's MCP connector and Claude Code SDK both push developers toward explicit `allowedTools` / allowlist patterns rather than a single coarse permission mode.
- Those hosted patterns are server-centric, but the underlying lesson generalizes: approval policy belongs in the runtime control plane, while the final UX for unresolved approvals belongs in the host.
- Argument-aware matching over canonical JSON pointers is a local substrate extension rather than a direct copy of any one hosted API. It is useful for local tools like `bash`, `read`, or `web_fetch`, where "same tool name" is still too coarse to express safe auto-allow rules.

Implementation impact:

- `agent-core-runtime` now exposes `ToolApprovalPolicy` alongside `ToolApprovalHandler`.
- Hosts can install ordered first-match rules that `allow`, `ask`, or `deny` based on tool name, tool origin, and selected argument predicates (`exists`, `value equals`, `string matcher`, `URL host matcher`).
- The runtime consults policy before invoking the approval handler. Policy can suppress hint-driven approval, force review for otherwise safe tools, or deny the call outright.
- Hook-driven permission denial or review still wins over policy suppression, so higher-order host policy is not erased by a local allowlist shortcut.

### 17. Prompt-cache controls should be typed provider request options, not opaque JSON folklore

Reason:

- OpenAI's prompt caching guide is explicit that cache hits depend on exact prefix reuse, optional `prompt_cache_key`, and a request-level `prompt_cache_retention` policy (`in_memory` vs `24h`).
- Those knobs are provider-specific request controls, not runtime semantics. Exposing them only through a free-form `additional_params` JSON blob makes host code brittle and hides intent.
- The runtime still needs stable preambles, append-only history, and deterministic tool ordering for cache reuse, but the concrete retention and cache-routing options belong in the provider adapter.

Implementation impact:

- `agent-core-rig` now exposes typed OpenAI request controls for `prompt_cache_key` and `prompt_cache_retention`.
- The adapter merges those controls into OpenAI request payloads while leaving non-OpenAI providers unchanged.
- If a host also passes `additional_params`, the typed cache controls are merged at the top level instead of forcing the host to handcraft provider JSON for a common optimization path.

### 18. JSONL transcripts should be paired with a small mutable index and maintenance policy

Reason:

- OpenClaw's session design separates a small mutable session store from append-only transcript JSONL, and its maintenance controls bound transcript artifacts by age, entry count, and disk use.
- OpenClaw's memory/QMD indexing notes also reinforce a practical pattern: keep disk transcripts as the source of truth, then build a lighter index/summary layer that can be rebuilt if it drifts.
- For an agent substrate, the main risk with raw JSONL-only storage is not durability but operational cost: every list/search call becomes a full scan, and old runs accumulate forever unless hosts reimplement pruning themselves.

Implementation impact:

- `FileRunStore` now keeps append-only `*.jsonl` transcripts as the durable record and writes a separate `runs.index.json` sidecar containing run summaries, lightweight search corpus text, and session-id bookkeeping.
- On open, the store validates the sidecar against the transcript files and rebuilds it when the file set drifts, so crashes between transcript append and index write recover cleanly by rescanning disk.
- List operations now read summaries from the sidecar, while search uses the sidecar as a prefilter before replaying candidate runs for exact preview generation.
- Hosts can now opt into retention by maximum run count or maximum run age without changing the transcript contract or introducing a heavier mandatory database dependency.

### 19. OpenAI Responses state should be modeled as provider continuation, not hidden backend trivia

Reason:

- OpenAI's conversation-state guide is explicit that `previous_response_id` is a first-class continuation primitive, and that follow-up turns should pass only the new user input when that chain is active.
- The same guide also notes that Responses `instructions` are request-level guidance rather than ordinary chat items. Re-encoding fixed instructions as system messages during `previous_response_id` chaining would accumulate duplicate prompt state instead of replacing the active top-level instructions for the next turn.
- OpenAI's compaction guide distinguishes between local/manual transcript summarization and server-side `context_management` compaction. The server-side path works best when the runtime keeps the upstream response chain intact, while the standalone `/responses/compact` path returns opaque compacted items that should be passed forward as-is.
- OpenClaw's session and compaction notes reinforce the substrate lesson here: local append-only transcript history remains the durable source of truth, but provider-side continuation state can still be used as a transport optimization and should be reset explicitly when the local visible-history shape changes.

Implementation impact:

- `agent-core-types` now carries `ProviderContinuation` in `ModelRequest` and `ModelEvent::ResponseComplete`, so provider-managed state is explicit at the runtime boundary instead of being smuggled through opaque metadata.
- `agent-core-runtime` now tracks the last provider continuation plus a transcript cursor. When provider-managed history is enabled, runtime sends only append-only transcript growth after the last successful upstream response; if the provider reports `previous_response_not_found`, runtime clears the chain and retries once with the full visible transcript.
- The OpenAI adapter now uses a native Responses streaming request path so it can preserve `response_id`, send top-level `instructions`, keep `prompt_cache_*` controls intact, and attach `context_management` compaction hints without waiting for generic adapter support.
- Local runtime compaction now explicitly resets provider continuation state, because once runtime rewrites the visible request window into a new summary/tail boundary, the old upstream response chain no longer matches the local context shape.

## Remaining Gaps

- MCP prompt arguments are discovered and displayed, but the TUI does not yet provide an argument-entry UX beyond the current basic command surface.
- Approval policy is no longer purely coarse, but persistent allowlists, config-driven rule loading, and richer host-side policy distribution are still missing.
- The optional local `web_search` path is a bootstrap implementation. It does not yet match hosted-tool quality for ranking, citation richness, or location-aware search controls.
- Optional `web_fetch` does not yet parse binary documents like PDFs into model-friendly text.
- The new skill model removes heuristic activation, but richer explicit skill policy and versioning still need to be designed.
- OpenAI server-side compaction hints are now supported on the create path, but the standalone `/responses/compact` output window is still not preserved as first-class opaque runtime transcript items.
