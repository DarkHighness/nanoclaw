# Tool Interface Design

This note focuses on the contract shape of local coding tools: their parameters, their output format, and the invariants that let models move from discovery to safe edits without losing their anchors.

## Why This Pass Exists

The earlier tool surface had a real mismatch:

- `read` returned unnumbered plain text slices
- `edit` expected exact `old_text` / `new_text` replacements over the raw file body

That shape worked for trivial cases, but it did not give the model a stable transition from:

1. locating relevant lines
2. reasoning over those lines
3. issuing an edit that can be validated against what it previously saw

For coding agents, that gap matters more than whether a tool "basically works."

## External Baselines

### Industrial baselines

- Anthropic's text editor tool splits file work into `view`, `str_replace`, `insert`, and `create`, and its `view` output is line-oriented and line-numbered. That is a strong signal that reading and editing should share the same line-based mental model instead of treating reads as free-form blobs.
- OpenAI's `apply_patch` tool pushes larger edits toward structured diffs rather than ever-larger raw string replacements. That suggests a two-tier edit surface: precise local edit commands for small changes, and patch-oriented editing for larger multi-file changes.
- OpenCode keeps `read` as a line-range-oriented file view, `edit` as exact replacement, and `patch` as a separate higher-power mutator. That separation is useful: `write` should not absorb all mutation use cases, and `patch` should not replace every local edit.

### Research baselines

- Trace-Free+ argues that tool descriptions and parameter schemas are not secondary polish; they are part of agent performance, especially when the model must choose among many tools.
- Tool-Genesis shows that small interface defects and underspecified contracts get amplified downstream. A weak tool interface can look like a reasoning failure even when the root cause is contract ambiguity.
- OpaqueToolsBench shows that real tools are often underspecified, and that execution feedback should refine the model's understanding of tool behavior. This argues for explicit failure messages and visible edit guards.
- ToolComp argues that process quality matters, not just end-task success. For coding tools, that means the runtime should preserve enough intermediate structure to evaluate whether a tool call is correctly grounded.
- LocAgent and related repo-level SWE work reinforce that line-level and entity-level localization quality directly affects downstream patch success. File tools should make those references explicit.

## Design Principles

### 1. Discovery and mutation should be separate stages

- `list`, `glob`, and `grep` should answer "where should I look?"
- `read` should answer "what exactly is there right now?"
- `edit` should answer "how do I change this exact region?"
- `write` should answer "how do I create or intentionally replace the whole file?"
- `patch` should answer "how do I apply a larger staged multi-file diff?"

Do not make `write` the universal edit escape hatch, and do not force every multi-file change through `edit`.

### 2. Reads must produce stable anchors

The primary read contract should expose:

- path
- visible line range
- total line count
- stable file snapshot id
- stable slice hash for the returned selection

The human-readable portion should be line-numbered by default, because line numbers are the anchor most coding agents naturally reason over.

### 3. Mutations should accept optional freshness guards

Mutating tools should be able to reject stale follow-up calls. In practice that means:

- `expected_snapshot` for whole-file freshness
- `expected_selection_hash` for range-oriented edits

These should be optional, not mandatory, because not every provider reliably reuses metadata. But the default host policy should encourage them.

### 4. Line-oriented edits and string-oriented edits should coexist

No single edit primitive is sufficient.

- exact string replace is efficient when the model has a precise snippet
- line-range replacement is better when the model is reasoning from numbered views
- insertion after a line is simpler than forcing the model to reconstruct surrounding context

That is why the local `edit` tool now supports:

- `str_replace`
- `replace_lines`
- `insert`

### 5. Do not prefix every line with a checksum

There is a real temptation to add Cursor-like or editor-like per-line consistency hashes. I did not find a primary source documenting Cursor's exact contract, so I am treating that pattern as an inferred product idea rather than a sourced requirement.

For this substrate, per-line hashes are not the best default:

- they add persistent token noise to every read
- they become visually dominant in long code listings
- they are unstable under line insertion and deletion
- they encourage the model to overfit on a UI artifact rather than the code span itself

The current design instead uses:

- line numbers for human and model grounding
- one file snapshot id for coarse freshness
- one slice hash for the selected view

This keeps the output compact while still giving the mutating tool a stale-read guard.

## Current Contract

### `read`

Inputs:

- `path`
- `start_line`
- `end_line`
- `line_count`
- `annotate_lines`
- `anchor_text`
- `anchor_context`
- `anchor_occurrence`
- `anchor_ignore_case`

Compatibility aliases are preserved for now:

- `offset` -> `start_line`
- `limit` -> `line_count`

Outputs:

- header with `path`, `lines`, `snapshot`, and `slice`
- line-numbered body by default
- optional anchor hint showing which matched line anchored the span
- continuation hint when more lines remain
- metadata containing the same ids for host-side auditing

### `edit`

Inputs:

- `path`
- `command`
- `expected_snapshot`
- `expected_selection_hash`

Command-specific fields:

- `str_replace`: `old_text`, `new_text`, optional `replace_all`
- `replace_lines`: `start_line`, `end_line`, `text`
- `insert`: `insert_line`, `text`

Compatibility path:

- omitting `command` while providing `old_text` / `new_text` still resolves to `str_replace`

Outputs:

- concise operation summary
- before/after snapshot ids
- machine-readable metadata for host logging

### `write`

Inputs:

- `path`
- `content`
- `if_exists`
- `if_missing`
- `expected_snapshot`

Outputs:

- concise create/replace summary
- before/after snapshot ids
- machine-readable metadata describing create-vs-overwrite and byte count

The important design choice here is that `write` is now explicit about missing/existing-file policy. It is still a full-file mutator, but it no longer has to behave like a silent unconditional overwrite.

### `patch`

Inputs:

- `operations`

Each operation is one of:

- `write`
- `edit`
- `delete`
- `move`

`patch` stages those operations in memory first, then commits them only if all operations succeed. That keeps the tool surface aligned with the multi-file diff role suggested by OpenAI and OpenCode, without forcing the runtime into partial-apply behavior.

The contract now also emits:

- operation-indexed diagnostics on failure (`failed_operation_index` plus compact failed input shape)
- per-file hunk-style previews for changed files, capped to a small removed/added line budget
- explicit move semantics (`from_path` -> `to_path`) with source snapshot guards and destination overwrite policy

### `list`, `glob`, and `grep`

These discovery tools now follow the same pattern:

- stable text header in the human-readable output
- bounded result counts
- structured metadata arrays (`entries` / `matches`)
- explicit truncation and limit flags
- deterministic file traversal order for stable repeated calls
- `grep` de-duplicates overlapping context rows so each file+line appears once

That gives the model and the host a cleaner transition from discovery to `read`.

### `bash`

`bash` remains an open-world tool, but its contract is tighter now:

- explicit execution modes: `run`, `start`, `poll`/`continue`, and `cancel`
- long-running commands return a `session_id` and can be polled without rerunning
- `poll` supports `stdout_start_char` / `stderr_start_char` windowing so follow-up calls can continue from known offsets
- bounded output via `max_output_chars` in both one-shot and poll mode
- explicit `timeout_ms` for each command session
- optional per-call environment overrides
- status metadata (`running`, `completed`, `timed_out`, `cancelled`) plus exit-code/error fields

That keeps the substrate minimal while still matching the asynchronous/pollable execution shape used by industrial agent loops.

### `web_search` and `web_fetch`

The optional web tools now mirror the local file tools more closely:

- `web_search` supports per-call domain filtering plus explicit pagination (`limit`, `offset`) and stable per-result ids for follow-up retrieval/citation wiring
- `web_fetch` supports `start_index` continuation and now returns a stable `document_id` with optional `expected_document_id` guard for stale-read detection across follow-up calls
- both tools now expose stronger provenance metadata (`retrieved_at`, response headers, result domains/ranks) so hosts can audit retrieval behavior

### `todo_read`, `todo_write`, and `task`

The agentic tools now also participate in the same grounding model:

- `todo_read` returns a stable revision id
- `todo_write` accepts an optional `expected_revision` plus `replace` / `merge` modes
- `task` prefers explicit `prompt` / `agent` inputs, while preserving legacy aliases for compatibility
- `task` now normalizes subagent output into machine-readable `status`, `summary`, and `artifacts` metadata while still preserving full text output for transcript continuity

These are not file tools, but the same principle applies: reads should expose a stable anchor, and writes/delegations should accept enough structure to validate follow-up actions.

## Sources

- OpenAI Apply Patch: [developers.openai.com/api/docs/guides/tools-apply-patch](https://developers.openai.com/api/docs/guides/tools-apply-patch/)
- OpenAI Codex CLI features: [developers.openai.com/codex/cli/features](https://developers.openai.com/codex/cli/features/)
- OpenAI background mode guide: [platform.openai.com/docs/guides/background](https://platform.openai.com/docs/guides/background)
- OpenAI tools overview: [platform.openai.com/docs/guides/tools?api-mode=responses](https://platform.openai.com/docs/guides/tools?api-mode=responses)
- Anthropic text editor tool: [docs.anthropic.com/en/docs/agents-and-tools/tool-use/text-editor-tool](https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/text-editor-tool)
- Anthropic SDK overview: [docs.anthropic.com/en/docs/claude-code/sdk](https://docs.anthropic.com/en/docs/claude-code/sdk)
- OpenCode tools: [opencode.ai/docs/tools](https://opencode.ai/docs/tools/)
- MCP progress utility: [modelcontextprotocol.io/specification/draft/basic/utilities/progress](https://modelcontextprotocol.io/specification/draft/basic/utilities/progress)
- Trace-Free+: [arXiv:2602.20426](https://arxiv.org/abs/2602.20426)
- Tool-Genesis: [arXiv:2603.05578](https://arxiv.org/abs/2603.05578)
- OpaqueToolsBench: [arXiv:2602.15197](https://arxiv.org/abs/2602.15197)
- ToolComp: [arXiv:2501.01290](https://arxiv.org/abs/2501.01290)
- LocAgent: [arXiv:2503.09089](https://arxiv.org/abs/2503.09089)
