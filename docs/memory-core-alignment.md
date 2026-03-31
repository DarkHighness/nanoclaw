# Memory Core Alignment

Date: 2026-03-31

Status: Active Design Note

## Goal

Align `memory-core` with the most important Claude-style memory capabilities
without changing the repository's existing workspace-local state model.

This slice intentionally aligns capability shape, not storage location:

- project instructions should be discoverable as long-lived procedural memory
- managed auto memory should expose one concise index file plus topic files
- durable memories should carry typed hooks (`type` + `description`) that are
  usable at recall time, not just stored as passive metadata
- the local default backend should stay offline-first and Markdown-grounded

`nanoclaw` keeps mutable memory state under `.nanoclaw/memory/` instead of
moving it to a machine-global home directory. That remains the repository's
host-owned boundary for derived indexes, runtime exports, and managed memory.

## Capability Mapping

Claude-style concept -> `nanoclaw` implementation

- `CLAUDE.md`
  - `AGENTS.md` and nested `**/AGENTS.md`
  - indexed as `procedural` memory with instruction-oriented layers
- auto memory index
  - `.nanoclaw/memory/MEMORY.md`
  - generated from managed memory as a concise hook index
  - capped to 200 lines / 25 KB so it stays primer-sized
- auto memory topic files
  - `.nanoclaw/memory/procedural/**/*.md`
  - `.nanoclaw/memory/semantic/**/*.md`
- runtime and subagent memory
  - remain in the existing layered tree under `.nanoclaw/memory/episodic/`,
    `.nanoclaw/memory/working/`, and `.nanoclaw/memory/coordination/`

## Index Semantics

The generated `.nanoclaw/memory/MEMORY.md` is derived state.

It is intentionally concise:

- durable procedural and semantic notes are listed explicitly
- each durable entry is rendered as a Claude-style hook line:
  `- [Title](path) — one-line description`
- transient episodic, working, and coordination memory is summarized instead of
  being expanded in full

That keeps the file useful as a future session-primer input while avoiding a
second giant bucket of duplicated runtime text.

## Retrieval Implications

The generated auto-memory index is searchable, but `memory-core` treats it like
other summary files and slightly downranks it relative to the specific topic
file it points at. The index exists to help navigation and lightweight recall,
not to replace the source note.

Recall now also indexes durable memory labels:

- explicit Markdown titles when present
- durable `description` hooks from frontmatter
- durable `type` taxonomy (`user`, `feedback`, `project`, `reference`)

That mirrors the part of Claude's recall flow that first narrows memory files
using concise labels before reading full note bodies.

## Startup Primer

`code-agent` now consumes a lightweight startup primer from the workspace-local
memory surface.

It injects concise excerpts from:

- `AGENTS.md`
- `MEMORY.md`
- `.nanoclaw/memory/MEMORY.md`

The primer is intentionally static for one runtime session and trimmed to keep
base instructions bounded.

Refresh semantics are cache-oriented:

- initial boot builds the primer once
- `StartFresh` rebuilds it once before the next root session starts
- `ResumeAgentSession` rebuilds it once before the reattached session starts
- normal turns do not recompute or mutate base instructions

That keeps workspace memory aligned with Claude's session-boundary behavior
while avoiding per-turn base-instruction churn that would degrade prefix-cache
reuse.

## Query-Time Recall

Normal turns may still inject memory recall, but not through base instructions.

The runtime now treats query-time recall as a separate synthetic message that is
prepended before the operator's original user message:

- current-session working memory is consulted first when available
- recall is best-effort and timeout-bounded
- recall is inserted as its own transcript message
- the operator prompt remains a separate message with unchanged bytes
- hooks, session events, and prompt previews continue to observe the original
  user request, not a merged recall blob

This boundary matters for both auditability and cache behavior. The recall path
must not rebuild base instructions per turn, and it must not collapse recall
into the same user message that the operator actually typed.

Because `memory-core` lexical retrieval uses strict token conjunction, the
augmentor shapes natural-language questions into concise content-term queries
before searching. That keeps prompts like "Should I use a canary deploy before
restart?" recallable without requiring the memory files to literally contain
every conversational filler token.

Durable recall now uses a more Claude-like two-stage path:

- working memory remains search-first so the active session note can win on the
  current task state
- procedural and semantic memory first go through a header-level selector that
  ranks candidate files using title, description, type, tags, and path labels
- only the selected durable files are read back for short snippets
- primer-style entrypoints such as `AGENTS.md`, `MEMORY.md`, and the generated
  auto-memory index are excluded from this durable selector because they are
  already part of the stable startup context
- if header selection finds nothing useful, the augmentor falls back to body
  search so content-only memories do not become unreachable

## Compaction Snapshots

Claude-style memory continuity is not only about durable recall. It also relies
on a compacted session keeping a usable working-memory handoff for the next
turns and for later resume.

`nanoclaw` now persists that handoff as working memory:

- when conversation compaction completes, the host writes the latest compact
  summary into `.nanoclaw/memory/working/sessions/<session>.md`
- that write replaces the previous continuation snapshot for the same target so
  the file stays a bounded latest-state handoff instead of an append-only log
- the target stays stable across agent-session rotation, so repeated compaction
  updates one session note instead of scattering snapshots across many files
- the record is tagged as a continuation snapshot for the current runtime
  session
- later query-time recall checks this working scope before consulting durable
  procedural and semantic memories

That keeps post-compaction continuity in the memory system itself instead of
depending only on one synthetic transcript summary message.

The compactor now also asks for Claude-style session-memory headings so the
persisted working note has a more stable structure:

- `Session Title`
- `Current State`
- `Task specification`
- `Files and Functions`
- `Workflow`
- `Errors & Corrections`
- `Codebase and System Documentation`
- `Learnings`
- `Key results`
- `Worklog`

The host now renders those snapshots into a fixed session-note skeleton instead
of persisting free-form Markdown directly:

- `.nanoclaw/memory/working/sessions/<session>.md` always keeps the full Claude
  section list plus the per-section italic guidance lines
- recognized compaction headings are mapped into their matching sections
- if compaction returns free-form text instead, the host falls back to
  `Current State` so continuity still survives
- later snapshot updates still use replace semantics, but they now replace the
  content inside one stable note shape rather than swapping between arbitrary
  summary layouts

Compaction now also exposes two host-visible message boundaries:

- `compacted_through_message_id`: the last pre-compaction message that was
  folded into the summary window
- `summary_message_id`: the synthetic summary message that replaced that window

The host uses the summary boundary, not the raw source boundary, when it
calculates later transcript deltas for incremental session-note refreshes.
That avoids replaying already-compacted transcript text after the summary has
become the new visible history anchor.

## Incremental Session Notes

Claude-style session memory is not only refreshed at compaction time.

`nanoclaw` now also performs bounded incremental updates to the structured
session note using an internal maintenance request:

- the first incremental refresh starts once the visible context reaches 10,000
  tokens
- later refreshes happen after another 5,000 context tokens or 3 tool calls
- the refresh runs in a background sidecar task instead of blocking the main
  turn completion path
- only one refresh may be in flight for a session at a time; stale in-flight
  work is abandoned after 60 seconds so later turns can recover
- the update request only receives transcript entries that were not already
  covered by the last summary boundary
- the model must return the full note while preserving the host-owned section
  skeleton and italic guidance lines exactly
- refreshed notes still use replace semantics in
  `.nanoclaw/memory/working/sessions/<session>.md`
- the update prompt now reminds the model to keep each section under roughly
  2,000 tokens and the full note under roughly 12,000 tokens
- when the existing note is already over those limits, the update prompt points
  at the specific oversized sections so the next refresh condenses them instead
  of only appending more detail

The session note now persists that boundary in its own frontmatter:

- every persisted session note carries `last_summarized_message_id`
- incremental refresh writes the latest visible message id that the refreshed
  note now covers
- compaction snapshots write the synthetic `summary_message_id` they produced
- resume and cold-start recovery now reload that durable boundary instead of
  guessing coverage from the current visible transcript tail

This keeps the session note closer to Claude's continuously-maintained working
memory without pushing note maintenance into base instructions or per-turn
prompt-prefix churn, and it mirrors Claude's "background extraction with
bounded stale recovery" shape more closely than a synchronous post-turn write.

## Session-Memory Compaction

Claude Code does not only refresh session memory in the background. When
compaction chooses the session-memory continuity path, it first gives any
in-flight extraction a bounded chance to finish and then uses the maintained
session note as the continuity summary itself.

`nanoclaw` now mirrors that shape in the compactor layer:

- the root runtime compactor is now a wrapper around the existing model
  compactor instead of a single hard-coded model-summary path
- before compacting, the wrapper gives the current session-note refresh up to
  15 seconds to finish, but stops waiting if the refresh is already older than
  60 seconds
- if the structured session note exists and its
  `last_summarized_message_id` covers the source window being compacted, the
  note body itself becomes the new compaction summary
- if the note is missing, empty, stale relative to the current compact source,
  or the operator supplied explicit `/compact` notes, the wrapper falls back to
  the normal model compactor
- before a note becomes the compaction summary, oversized sections are truncated
  at line boundaries to roughly the same 2,000-token per-section budget Claude
  uses, and the summary points back to the full note path when truncation
  happened

This keeps compaction continuity aligned with Claude's "session memory as the
summary source" path without forcing every compaction to depend on a perfect
background refresh, and it lets both auto-compaction and manual compaction
reuse the same bounded-wait decision point.

The runtime-side retained tail now also keeps a more Claude-like continuity
window around the summary boundary:

- the retained tail still honors the message-count floor configured by the
  runtime profile
- large transcripts expand that tail by whole request rounds until it carries
  at least a modest token and text-message floor
- the split point is aligned to an explicit request-round boundary so
  compaction does not keep only an assistant reply, only the tail half of a
  synthetic recall + user prompt pair, or a prompt without the immediately
  preceding steer/reminder system message that framed it
- tool call / tool result pairs remain indivisible across that boundary

That keeps the post-compact transcript closer to Claude's "complete recent
trajectory" shape instead of preserving an arbitrary suffix of message slots.

Resume and transcript restore now apply the same invariant when they rebuild a
compacted runtime session:

- modern checkpoints already carry the exact retained tail message ids, so
  resume reproduces the same visible order as the live runtime
- older checkpoints that kept only the tail end of a request round are now
  upgraded during reconstruction by backfilling the missing request-side
  `system/user` prefix ahead of the first retained message

That keeps resumed sessions closer to the live post-compact context shape
instead of reviving a mid-round suffix that the current runtime would no longer
preserve.

Interrupt-driven restarts now reuse the same request-side cluster shape:

- rollback no longer drops only the final user prompt from an active turn
- if the interrupted turn started with steer/reminder or synthetic recall
  context, that request-side prefix is removed together with the follow-through
  assistant trajectory
- compacted summaries and retained tails stay stable because the rewind walks
  visible transcript order instead of raw transcript indices

That keeps restarts aligned with Claude's "replace the active round" behavior
instead of leaving behind half of the interrupted request context.

Manual history rollback now consumes runtime-projected request rounds instead
of rebuilding turns from raw visible messages in the TUI:

- the rollback anchor can start at a request-side `system` message when that
  system prefix framed the user prompt that should be restored
- the restored composer draft still comes from the latest user prompt inside
  that request-side cluster
- compacted summaries are excluded from rollback anchors by the runtime
  projection instead of by frontend heuristics

That keeps operator-driven rewinds aligned with the same round boundaries used
by compaction, resume, and interrupt restarts.

Persisted agent-session history views now reuse the same reconstructed visible
transcript projection:

- compacted agent sessions are loaded through the persisted runtime-session
  checkpoint path before the host renders transcript history
- when a checkpoint predates resume metadata, the host still falls back to raw
  replay so history inspection stays available

That keeps `/agent_session` closer to the live runtime transcript shape
without breaking older stored sessions.

Whole-session history and transcript export now compose those same per-agent
visible transcript slices in first-seen agent-session order:

- compacted root-agent windows stay compacted in `/session`
- later root-agent rotations append their own visible transcript windows
- transcript export no longer revives pre-compaction suffixes that the live
  runtime has already replaced with a summary boundary

That keeps operator-facing history exports closer to what the runtime actually
showed during the session instead of replaying every hidden transcript node.

Session list, search, and memory-export summaries now use that same visible
transcript accounting for their message counts:

- persisted `transcript_message_count` values no longer report hidden
  pre-compaction replay nodes
- file-store sidecars rebuild on compaction checkpoints so cached summaries stay
  aligned with the live visible window
- memory exports keep their transcript counts consistent with the post-
  compaction history shape they summarize

Session search is now aligned to the same operator-visible boundary:

- transcript text matches come only from the visible post-compaction window
- structural event metadata such as tool names, task ids, and notifications
  remains searchable
- raw model-request payloads, hidden assistant bodies, and hook-injected prompt
  text no longer make compacted-away transcript content look searchable again

That keeps summary metadata aligned with the operator-facing transcript surface
instead of mixing visible content with raw replay-only counts.

Search ordering now also follows a more Claude-like metadata-first shape:

- last-user-prompt hits sort ahead of transcript-only hits
- session-id hits sort ahead of transcript-only hits
- structural metadata hits sort ahead of visible transcript-body hits when both
  are present
- preview snippets follow the same priority, with prompt and metadata cues
  emitted before transcript excerpts

That keeps session search closer to an operator selector with concise cues,
instead of ranking sessions mainly by how many times the query string happened
to appear in transcript text.

## Side Questions (`/btw`)

Claude Code exposes side questions as a separate lightweight query path rather
than as a normal turn in the main transcript.

`nanoclaw` now mirrors that behavior with `/btw`:

- `/btw <question>` runs even while the main turn is still working
- it launches a separate operator-side request instead of interrupting the main
  runtime
- the request reuses the latest stable base instructions and visible transcript
  snapshot from the main session
- the wrapper prompt explicitly forbids tool calls and follow-up action promises
- the answer returns as its own inspector view instead of mutating the main
  conversation turn

This keeps the cacheable prefix close to the parent session while preserving a
clear boundary between "main work" and "side question" behavior.
