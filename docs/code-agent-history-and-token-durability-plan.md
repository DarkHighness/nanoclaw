# Code Agent History And Token Durability Plan

Date: 2026-04-10

Status: Active

## Goal

Repair `code-agent` persistence so that submitted prompt history keeps full
prompt context instead of collapsing to plain text, and session-level token
usage is durably available at summary, list, and search time without replaying
 whole sessions.

## Problems

1. `apps/code-agent/src/frontend/tui/input_history.rs` persists only `{ text }`,
   so prompt history loses attachment context and submission metadata.
2. The TUI submission path captures rich composer state in memory, but the
   durable event boundary collapses it to a string through
   `SessionEventKind::UserPromptSubmit` in `crates/types/src/event.rs`.
3. Prompt history durability and runtime session history are currently split and
   both lossy, so fixing only the TUI would still leave persisted session
   history incomplete.
4. Session token usage already exists in runtime events and
   `SessionTokenUsageReport`, but `crates/store/src/traits.rs` `SessionSummary`
   does not carry token fields.
5. `crates/store/src/file/index_sidecar.rs` persists summary, count, and search
   metadata only, so list, search, and summary reads cannot show token totals
   without replay.
6. `apps/code-agent/src/frontend/tui/history.rs` can show token usage only when
   loading a full session, which leaves summary-only reads incomplete.

## Constraints

- keep backward compatibility for existing prompt history files
- keep backward compatibility for persisted session events that encoded
  `UserPromptSubmit` as a plain string
- do not depend on replaying full session events for normal summary, list, and
  search token reads
- preserve append-only event history semantics
- prefer additive schema evolution over destructive migration
- treat prompt richness and token durability as one repair, because both are
  persistence-boundary problems

## Target State

1. A submitted prompt is represented by one durable payload that includes prompt
   text plus persisted attachment context.
2. The TUI input history file stores the same durable prompt snapshot, or a
   lossless subset of it, so restored history matches what was submitted.
3. `SessionEventKind::UserPromptSubmit` writes the rich payload for new events
   and still reads legacy string payloads.
4. `SessionSummary` carries durable token totals for summary, list, and search
   callers.
5. The file-store sidecar persists those token totals directly.
6. Existing sidecars missing token totals are rebuilt once and rewritten in the
   new format.
7. TUI history and search views consume summary-level token data directly while
   full-session inspection continues to work.

## Schema Changes

### Prompt submission payload

`crates/types/src/event.rs`

- replace the plain-string `UserPromptSubmit` payload with a rich serializable
  payload such as `SubmittedPromptSnapshot { text, attachments }`
- keep a legacy read path so old string payloads decode as a snapshot with
  `attachments = []`
- new writes emit only the object form

### Prompt history entry

`apps/code-agent/src/frontend/tui/input_history.rs`

- expand the persisted history entry schema from `{ text }` to a rich entry that
  stores `text` plus serialized attachment context
- keep missing attachment fields defaulting to empty for old files

### Session summary token durability

`crates/store/src/traits.rs`

- add durable token fields to `SessionSummary`
- prefer a compact summary-facing token snapshot if the full
  `SessionTokenUsageReport` is too heavy for summary storage

### Sidecar summary projection

`crates/store/src/file/index_sidecar.rs`

- extend the sidecar schema to persist the summary token snapshot alongside
  existing summary and search metadata
- bump the sidecar schema version and rebuild old sidecars once

## File Changes

- `crates/types/src/event.rs`
  - define the durable rich prompt submission payload
  - add legacy decoding support
  - add normalized accessors so callers read one shape
- `apps/code-agent/src/frontend/tui/state.rs`
  - ensure `ComposerSubmission` exposes a serializable prompt snapshot derived
    from the composer draft, including attachments
- `apps/code-agent/src/frontend/tui/mod.rs`
  - update `record_submitted_prompt` and the submission event path to use the
    rich prompt snapshot
- `apps/code-agent/src/frontend/tui/input_history.rs`
  - upgrade the history file schema
  - load old entries with empty attachments
  - persist new rich entries
- `apps/code-agent/src/frontend/tui/history.rs`
  - read summary-level token usage for session list, search, and summary lines
- `apps/code-agent/src/backend/session_catalog.rs`
  - project summary token fields through persisted session summary/search match
    types
- `crates/store/src/traits.rs`
  - extend `SessionSummary`
  - update summary builders and search helpers
- `crates/store/src/file/index_sidecar.rs`
  - project token usage into the sidecar
  - rebuild and rewrite older sidecars
- `crates/store/src/file.rs`
  - ensure list and search summary paths read and write the new token-bearing
    summary shape
- `crates/store/src/memory.rs`
  - keep in-memory summary recomputation aligned with the new `SessionSummary`
    contract
- `apps/code-agent/src/backend/session_history.rs`
  - keep loaded session and export surfaces aligned with the richer prompt event
    payload and summary token fields

## Compatibility And Migration

1. Old prompt history entries that only contain `{ text }` must deserialize into
   the new entry type with empty attachments.
2. Persisted session events must remain readable in both forms
   - legacy string payloads
   - new object payloads
3. New runtime writes should emit only the new object payload.
4. Existing sidecars without token usage fields should trigger one-time rebuild
   from durable session artifacts and then be rewritten in the new shape.
5. No offline migration is required before rollout. Reader compatibility and
   sidecar self-healing are sufficient.

## Implementation Steps

1. Add characterization tests for old prompt history files, legacy
   `UserPromptSubmit` event decoding, and current summary-level token gaps.
2. Introduce the shared durable prompt snapshot in `crates/types/src/event.rs`.
3. Wire the TUI submission path to that shared snapshot in
   `apps/code-agent/src/frontend/tui/state.rs` and
   `apps/code-agent/src/frontend/tui/mod.rs`.
4. Upgrade input history persistence in
   `apps/code-agent/src/frontend/tui/input_history.rs`.
5. Extend `SessionSummary` with durable token usage in
   `crates/store/src/traits.rs`.
6. Update sidecar persistence and rebuild behavior in
   `crates/store/src/file/index_sidecar.rs` and `crates/store/src/file.rs`.
7. Project the new summary token fields through
   `apps/code-agent/src/backend/session_catalog.rs` and
   `apps/code-agent/src/frontend/tui/history.rs`.
8. Run compiler-driven follow-through for every remaining constructor, matcher,
   and serializer touched by the type changes.

## Validation

- serde tests for old string `UserPromptSubmit` payloads and new object payloads
- prompt history tests for old-file read compatibility, new-file write shape,
  mixed-file loading, and attachment preservation
- store tests for token-bearing `SessionSummary` persistence, sidecar reload,
  and no-replay list/search reads
- TUI history tests that session list and search surfaces show token totals from
  `SessionSummary`
- a restart-oriented end-to-end durability test that proves rich prompt
  submissions and summary token usage both survive reload

## Risks And Rollback

- risk: changing `UserPromptSubmit` breaks old persisted events
  - mitigation: keep legacy decode support and test both wire formats
- risk: prompt history persists unstable UI-only state
  - mitigation: serialize only compact durable attachment metadata
- risk: sidecar schema drift breaks list/search on existing installs
  - mitigation: version the sidecar schema and rebuild once when fields are
    missing
- risk: duplicated token-fold logic causes mismatched totals
  - mitigation: derive summary token data from the same canonical fold already
    used by session-level reporting

## Atomic Commit Strategy

1. `docs(plan): add code-agent history and token durability plan`
2. `test(code-agent): characterize prompt history and event compatibility gaps`
3. `feat(types): add durable submitted prompt snapshot with legacy event decoding`
4. `feat(code-agent): persist rich prompt history entries from composer submissions`
5. `test(store): characterize missing summary token durability and sidecar upgrade cases`
6. `feat(store): persist token usage in session summaries and sidecar projections`
7. `feat(code-agent): read summary token usage in history list and search views`
8. `test(integration): cover legacy data upgrade and end-to-end durability paths`
