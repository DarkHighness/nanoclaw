# Tooling Industrial Alignment

This note turns the 2026-03-26 tooling comparison into implementation guidance for the local substrate.
It focuses on three contract surfaces that materially affect code-agent reliability:

- tool input contracts
- tool output contracts
- web discovery and retrieval

The question here is not whether the current tools "work." The question is whether they encode the same safety, grounding, and recovery properties that show up across current industrial agent stacks and the better tool-use papers.

## Status Markers

This note now keeps an explicit split between what has already shipped and what is still a gap.

### (a) Implemented In The Current Substrate

- File-tool inputs are already in the industrial envelope: grounded reads, explicit mutation modes, freshness guards, and staged patch application.
- `bash` already follows a handle-based execution model rather than one-shot rerun-only semantics.
- Runtime approval and approval-policy matching already live at the shared execution boundary instead of inside individual tools.
- `ToolName` is now a shared substrate type rather than a loose string, so approval rules, registry keys, provider mappings, and subagent allowlists all talk about the same semantic identifier.
- Optional `code-intel`, `web-tools`, and `agentic-tools` bundles compile against the same shared tool-identity contract instead of each carrying their own stringly-typed tool-name path.

### (b) Not Yet Implemented

- Tool outputs are still not first-class typed payloads with an explicit `output_schema`.
- Provider adapters still degrade most tool results to text summaries when crossing the provider boundary.
- Web retrieval is still bootstrap quality rather than production-quality retrieval.
- Redirect policy is still under-specified compared with the standard implied by current hosted stacks.
- The substrate still does not emit a fully typed host-facing tool event stream that makes transcript parsing optional.

### (c) Improvement Space

- Promote tool outputs to structured contracts, not just structured metadata.
- Move web extraction from regex-heavy cleanup to DOM/readability-based extraction with stronger provenance and citation support.
- Introduce a real pluggable search backend boundary so locale, freshness, and source mode become request semantics instead of backend folklore.
- Keep design notes like this one updated whenever a gap moves from “planned” to “implemented,” so the document remains an operational status note rather than a stale comparison.

## Scope And Baselines

The comparison below uses the current local implementation as the primary subject:

- local file tools in [`crates/tools/src/fs/`](../crates/tools/src/fs/)
- local web tools in [`crates/tools/src/web/`](../crates/tools/src/web/)
- shared tool types in [`crates/types/src/tool.rs`](../crates/types/src/tool.rs)
- runtime approval and execution flow in [`crates/runtime/src/`](../crates/runtime/src/)
- provider adapters in [`crates/provider/src/`](../crates/provider/src/)

The external baselines are the public parts of:

- MCP tools spec, especially `inputSchema`, `outputSchema`, `structuredContent`, `isError`, and tool annotations
- OpenAI Responses and Codex guidance, including typed tool items, background handles, shell/web tools, prompt caching, and citations
- OpenCode's local coding tool split (`read`, `edit`, `write`, `patch`, `websearch`, `webfetch`)
- Cursor's structured CLI output and MCP surface
- OpenClaw's typed tool model, context compaction, and pruning model
- research that treats tool descriptions, grounding quality, and process verification as first-order variables rather than UI polish

Where a product does not publish an exact internal implementation detail, this note treats the public contract shape as the baseline and avoids claiming hidden behavior as fact.

## Executive Summary

The current state is uneven:

- input contracts are mostly aligned with industrial best practice
- output contracts are usable for the current loop, but not yet structured enough for best-in-class orchestration
- web retrieval is safety-aware, but still too lightweight to count as industrial best practice

More concretely:

1. The local file and process tools already expose the right control points for safe iteration: line-grounded reads, explicit mutation modes, freshness guards, staged multi-file patching, and handle-based shell continuation.
2. The tool result model preserves `call_id`, `is_error`, and multipart content internally, but the substrate still lacks a first-class structured output contract and the provider adapters currently flatten most tool results to plain text.
3. The web tools preserve a useful search-vs-fetch split and basic outbound policy, but the search bootstrap, parsing strategy, provenance model, and redirect handling are still closer to a bootstrap implementation than a production retrieval layer.

## What "Best Practice" Means Here

Across OpenAI/Codex, Cursor, OpenCode, OpenClaw, MCP, and the cited papers, the same patterns show up repeatedly:

- discovery and mutation are separate tool stages
- reads return stable anchors that edits can refer back to
- mutations can reject stale follow-up calls
- long-running work uses stable handles instead of replaying the original request
- tool failures stay in-band so the model can recover
- tool outputs remain machine-readable all the way through provider round-trips
- web retrieval keeps source identity, citations, and policy enforcement attached to the fetched content
- safety checks happen at the shared runtime boundary, not as ad hoc tool-local prompts

That is the bar this note uses.

## Current Assessment

### 1. Input Contracts: Mostly Aligned

The strongest part of the current substrate is the input side of local tools.

What already matches the industrial baseline:

- `read` is line-oriented and emits stable anchors such as file snapshot ids and slice hashes.
- `edit`, `write`, and `patch` are separate tools instead of one overloaded mutator.
- `edit` supports more than one mutation mode, which keeps line-grounded and snippet-grounded edits on the same surface.
- mutation tools accept optional freshness guards instead of assuming the model is still operating on the latest read.
- `patch` stages multi-file work before commit, which avoids partial application on later failure.
- `bash` already uses explicit `run` / `start` / `poll` / `cancel` style execution rather than forcing the model to rerun long commands.
- approval lives in the runtime control plane, and policy can match on selected arguments rather than only on tool name.

Why that matters:

- Trace-Free+ and OpaqueToolsBench both reinforce that tool descriptions and argument schemas directly affect agent reliability.
- LocAgent-style results reinforce that line-level localization quality is not a UI detail; it is upstream of patch quality.
- AgentProcessBench-style findings reinforce that explicit recovery and verification steps matter because tool side effects are not free to replay.

Current judgment:

- `input` design is already within the normal industrial envelope.
- The main remaining work on `input` is incremental refinement, not redesign.

### 2. Output Contracts: Partially Aligned

The internal runtime model is better than the provider-facing contract.

What is already good:

- [`ToolResult`](../crates/types/src/tool.rs) preserves `call_id`, `is_error`, `metadata`, and multipart content.
- runtime tool failures stay in-band as error results instead of aborting the whole loop.
- local tools already emit structured metadata that the host can use for auditing, pagination, continuation, and stale-read detection.

What is still missing relative to current best practice:

- [`ToolSpec`](../crates/types/src/tool.rs) exposes `input_schema` and `output_mode`, but not a first-class `output_schema`.
- the core result shape does not distinguish between human-readable summary text and canonical structured payload in the MCP `structuredContent` sense.
- provider adapters currently collapse most tool results to text:
  - [`crates/provider/src/openai.rs`](../crates/provider/src/openai.rs) serializes tool output via `result.text_content()`
  - [`crates/provider/src/anthropic.rs`](../crates/provider/src/anthropic.rs) does the same for `tool_result`
- that flattening means non-text parts and richer output structure are lost across provider round-trips even though the substrate can represent them internally.

Why that is below the current bar:

- MCP now has an explicit `outputSchema` and `structuredContent` model.
- OpenAI's current Responses surface is typed at the item level, not just a text transcript.
- Cursor's CLI and event stream expose tool-call lifecycle and structured result fields directly instead of forcing every consumer to parse free-form text.
- OpenClaw's typed tool model assumes tool usage is part of the context budget and therefore worth keeping structured.

Current judgment:

- the internal model is close enough to extend
- the provider-facing output contract is not yet best-in-class

### 3. Web Search And Fetch: Not Yet Aligned

The current web layer gets several important boundaries right, but the parsing and provenance stack is still too thin.

What is already good:

- `web_search` and `web_fetch` are separate tools instead of hidden prompt behavior.
- both tools enforce HTTP(S)-only transport and block private/local hosts by default.
- `web_fetch` exposes continuation and freshness with `start_index` plus `document_id` / `expected_document_id`.
- `web_search` supports basic domain filtering and stable result ids.
- metadata already records useful provenance such as final URL, content type, and retrieval time.

What is still below best practice:

- [`crates/tools/src/web/search.rs`](../crates/tools/src/web/search.rs) hardcodes a Bing RSS bootstrap with `cc=us` and `setlang=en-US`, which is too rigid for locale-aware retrieval and too fragile to treat as a durable backend contract
- [`crates/tools/src/web/search.rs`](../crates/tools/src/web/search.rs) also parses RSS with regex helpers instead of a real parser or provider abstraction
- [`crates/tools/src/web/common.rs`](../crates/tools/src/web/common.rs) currently reduces HTML to text through regex stripping, which loses document hierarchy, links, code blocks, tables, and other structure that matters in technical retrieval
- the current flow exposes provenance metadata, but not model-visible citations or source annotations comparable to modern hosted web tools
- redirect handling is under-specified from a policy perspective because [`crates/tools/src/web/common.rs`](../crates/tools/src/web/common.rs) configures automatic redirect following while [`crates/tools/src/web/fetch.rs`](../crates/tools/src/web/fetch.rs) validates the original target before the request

That last point is the most urgent web-specific gap. A safe web policy has to apply to the actual destination, not just the first URL in the chain.

Current judgment:

- the current web tools are a useful bootstrap
- they should not be treated as an industrially complete retrieval subsystem yet

## Implementation Direction

The correct next step is not to rewrite everything at once. The current substrate already has the right boundaries. The work now is to strengthen the contracts that cross those boundaries.

### Priority 0: Make Tool Outputs First-Class Structured Data

This is the most important substrate-level gap because it affects every tool, not just web retrieval.

Required changes:

- add an `output_schema` field to `ToolSpec`
- extend tool results with an explicit structured payload alongside human-readable text
- preserve multipart tool content through provider adapters instead of collapsing everything to `text_content()`
- keep `is_error`, `call_id`, and metadata stable across local, provider, and MCP round-trips

Design constraints:

- the substrate still needs compact human-readable summaries because models reason well over text and hosts need readable transcripts
- the structured payload must be canonical enough that hosts can render it without parsing prose
- adapters may still degrade to text for providers that cannot carry richer structure, but that downgrade should happen only at the outer edge

Acceptance criteria:

- a tool can declare both `input_schema` and `output_schema`
- a tool result can carry both text summary and structured content without ambiguity
- OpenAI and Anthropic adapters preserve structured tool output when the provider surface supports it
- transcript replay does not lose the correlation between human-readable summary and structured payload

### Priority 0: Reapply Web Policy To Redirect Destinations

Before improving extraction quality, the web layer needs a stricter policy boundary.

Required changes:

- validate every redirect destination against the same allowlist, blocklist, and private-host policy used for the initial URL
- reject redirect chains that terminate outside policy
- make the rejection reason visible in the tool result instead of silently following the redirect

Acceptance criteria:

- a redirect from a public URL to a private or blocked host is rejected
- logs and tool metadata preserve both the requested URL and the rejected final destination
- search and fetch share the same redirect-policy behavior

### Priority 1: Replace Regex HTML Extraction With Structured Readability Extraction

The current extraction path throws away too much information for technical agent workflows.

Required changes:

- replace regex stripping with a DOM-based extraction pipeline
- preserve major headings, paragraphs, lists, links, code blocks, and tables in the extracted representation
- keep a compact text window for model consumption, but derive it from the structured extraction rather than from raw regex cleanup
- emit link and citation metadata in a stable machine-readable shape

Why this matters:

- OpenAI's hosted web tooling and Cursor's browser-aware workflows both assume the model can reason over retrieved structure, not just a flattened blob
- technical pages often depend on code fences, list nesting, headings, and link targets to remain intelligible

Acceptance criteria:

- an extracted article retains recognizable section boundaries
- technical documentation pages keep code blocks and link targets
- follow-up pagination works over the extracted representation, not over a lossy preprocessed string

### Priority 1: Introduce A Real Search Provider Boundary

The substrate should not couple the public search contract to one RSS bootstrap path.

Required changes:

- define a backend trait for search providers
- move Bing RSS bootstrap behind that trait as a fallback implementation, not as the contract
- make locale, freshness, and source-mode explicit request options instead of hardcoded query parameters
- preserve provider identity and retrieval mode in result metadata

Acceptance criteria:

- `web_search` keeps the same model-facing purpose while the backend becomes swappable
- the request contract can express locale-sensitive search behavior without environment-variable folklore
- result metadata makes it clear which backend and retrieval mode produced the result set

### Priority 2: Expose Host-Facing Typed Tool Events

The substrate already has runtime events, but tool output still leans too heavily on transcript text for downstream consumers.

Required changes:

- expose typed tool lifecycle events in a host-facing format that does not require prose parsing
- keep event identities stable across start, completion, failure, and cancellation
- align event fields with the same normalized ids already used by runtime and provider adapters

This is less urgent than structured tool results themselves, but it becomes much easier once the result contract is fixed.

## Non-Goals

This note does not recommend:

- turning the local web layer into a full browser automation stack
- forcing every provider to expose the same native JSON surface
- replacing readable text summaries with opaque machine payloads
- reworking the current file tool contract, which is already in the right shape

The goal is a stronger, more auditable substrate, not a product clone of any one hosted agent.

## Recommended Sequencing

The highest-leverage sequence is:

1. structured tool outputs
2. redirect-safe web policy
3. DOM-based fetch extraction
4. pluggable search backend and citations
5. host-facing typed event streaming

That order keeps the work substrate-first. It strengthens shared contracts before adding new retrieval sophistication on top.

## Sources

- MCP tools spec: [modelcontextprotocol.io/specification/2025-06-18/server/tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)
- OpenAI tools overview: [platform.openai.com/docs/guides/tools?api-mode=responses](https://platform.openai.com/docs/guides/tools?api-mode=responses)
- OpenAI web search guide: [platform.openai.com/docs/guides/tools-web-search?api-mode=responses](https://platform.openai.com/docs/guides/tools-web-search?api-mode=responses)
- OpenAI shell tool guide: [platform.openai.com/docs/guides/tools-shell](https://platform.openai.com/docs/guides/tools-shell)
- OpenAI background mode: [platform.openai.com/docs/guides/background](https://platform.openai.com/docs/guides/background)
- OpenAI prompt caching: [platform.openai.com/docs/guides/prompt-caching](https://platform.openai.com/docs/guides/prompt-caching)
- OpenAI Codex loop writeup: [openai.com/index/unrolling-the-codex-agent-loop](https://openai.com/index/unrolling-the-codex-agent-loop/)
- OpenCode tools: [opencode.ai/docs/tools/](https://opencode.ai/docs/tools/)
- OpenCode permissions: [opencode.ai/docs/permissions](https://opencode.ai/docs/permissions)
- Cursor agent overview: [cursor.com/docs/agent/overview](https://cursor.com/docs/agent/overview)
- Cursor CLI output format: [cursor.com/docs/cli/reference/output-format](https://cursor.com/docs/cli/reference/output-format)
- Cursor MCP docs: [cursor.com/docs/mcp](https://cursor.com/docs/mcp)
- OpenClaw tools: [docs.openclaw.ai/tools](https://docs.openclaw.ai/tools)
- OpenClaw context: [docs.openclaw.ai/context/](https://docs.openclaw.ai/context/)
- OpenClaw compaction: [docs.openclaw.ai/concepts/compaction](https://docs.openclaw.ai/concepts/compaction)
- OpenClaw session pruning: [docs.openclaw.ai/concepts/session-pruning](https://docs.openclaw.ai/concepts/session-pruning)
- Trace-Free+: [arXiv:2602.20426](https://arxiv.org/abs/2602.20426)
- OpaqueToolsBench: [arXiv:2602.15197](https://arxiv.org/abs/2602.15197)
- AgentProcessBench: [arXiv:2603.14465](https://arxiv.org/abs/2603.14465)
- LocAgent: [arXiv:2503.09089](https://arxiv.org/abs/2503.09089)
