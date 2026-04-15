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
  - use a four-level model: `procedural`, `semantic`, `working`, `episodic`
  - coordination records are now treated as specialized `working` layers even
    when they still live under the legacy `.nanoclaw/memory/coordination/`
    tree for backward-compatible reads

## Index Semantics

The generated `.nanoclaw/memory/MEMORY.md` is derived state.

It is intentionally concise:

- durable procedural and semantic notes are listed explicitly
- each durable entry is rendered as a Claude-style hook line:
  `- [Title](path) — one-line description`
- transient episodic and working memory is summarized instead of being
  expanded in full

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

Normal turns no longer receive host-injected recall messages.

Instead, memory lookup is model-driven:

- the system prompt tells the agent when prior workspace memory may matter
- the agent decides when to call `memory_search`, `memory_list`, and
  `memory_get`
- the operator prompt stays byte-for-byte unchanged instead of being prefixed
  with a synthetic host recall blob
- hooks, prompt previews, and runtime events continue to observe the original
  user request rather than a rewritten prompt

This boundary matters for both auditability and cache behavior. Recall policy
now lives in the prompt-plus-tool contract, not in a host-side per-turn
augmentor.

The active session has a separate raw-state path before any memory extraction:

- the visible transcript remains the immediate source of truth for the current
  live conversation
- the host session store still persists the full event stream even before
  compaction
- `memory-core` is the distilled layer, not the only place where session state
  exists

That means "not yet compacted" does not imply "lost". It only means the
conversation may still exist as raw session history rather than as a distilled
working-memory note.

## Compaction Snapshots

Claude-style memory continuity is not only about durable recall. It also relies
on the current session exposing a usable working-memory handoff for later
resume, interruption, and follow-up turns.

`nanoclaw` now treats that handoff as a shared working-session note with two
write paths:

- before compaction, the model may update the note itself through
  `memory_record(scope=working, layer=session, mode=replace)` when the task
  state changes in a handoff-worthy way
- at compaction time, the host still writes the latest compact summary into the
  same `.nanoclaw/memory/working/sessions/<session>.md` target as a fallback
  and reconciliation path

That distinction is deliberate:

- the note should not first appear only after compaction
- the host should not run token-threshold or tool-count heuristics to decide
  every ordinary refresh
- compaction remains the narrow host-owned moment where session continuity must
  be repaired even if the model never updated the note earlier

The shared session note still has stable replace semantics:

- `.nanoclaw/memory/working/sessions/<session>.md` stays the stable per-session
  working-memory location
- writes replace the previous note instead of appending to an ever-growing log
- the target stays stable across agent-session rotation, so one root session
  accumulates one bounded continuation note rather than many scattered
  snapshots

That keeps current-session continuity queryable inside the memory system
without requiring every recovery path to reconstruct meaning from raw
transcript alone.

The host-owned compaction path still normalizes notes into a stable structure:

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

When compaction writes or rewrites the note:

- `.nanoclaw/memory/working/sessions/<session>.md` always keeps the full Claude
  section list plus the per-section italic guidance lines
- recognized compaction headings are mapped into their matching sections
- if compaction returns free-form text instead, the host falls back to
  `Current State` so continuity still survives
- later snapshot updates still use replace semantics, but they now replace the
  content inside one stable note shape rather than swapping between arbitrary
  summary layouts

Compaction also remains the place where the host can attach a trustworthy
summary boundary:

- `compacted_through_message_id`: the last pre-compaction message that was
  folded into the summary window
- `summary_message_id`: the synthetic summary message that replaced that window

The session note persists that boundary in its own frontmatter:

- every persisted session note carries `last_summarized_message_id`
- host-written compaction snapshots write the synthetic `summary_message_id`
  they produced
- resume and cold-start recovery can reload that durable boundary instead of
  guessing coverage from the current visible transcript tail

## Model-Driven Session Notes

The important pre-compaction question is therefore not "should the host run a
periodic background summarizer?" but "how does the note come into existence
before the first compaction?"

The current answer is prompt-driven note maintenance:

- the system prompt tells the model not to wait for host compaction before
  preserving important session state
- when the current task gains state that would matter after resume,
  interruption, or handoff, the model should update the working session note
  itself with `memory_record(scope=working, layer=session, mode=replace)`
- if a session note already exists, the model can read it first and preserve
  useful sections while replacing stale content
- this keeps refresh policy inside the prompt contract instead of a host
  threshold policy based on token counts or tool counts

This is intentionally best-effort rather than absolute. If a process crashes
before the model updates its note, the raw session transcript still exists in
the session store, and later compaction or resume can reconstruct continuity
from there.

## Episodic Daily Capture

`memory_record` still supports `scope=episodic` with the append-only
`daily-log` layer at `.nanoclaw/memory/episodic/logs/YYYY/MM/YYYY-MM-DD.md`.

That layer remains available for explicit use, but the host no longer runs a
normal-turn background episodic extractor. Episodic capture is now tool-driven
in the same way as other non-compaction memory writes:

- the model can write explicit daily-log entries when the user asks to remember
  something or when a durable raw fact should be preserved for later
  consolidation
- the append-only constraint still prevents silent rewrite of previously
  captured facts

This keeps ordinary turn-by-turn extraction policy in the prompt/tool layer
instead of the host scheduler while preserving a raw episodic surface for
intentional use.

That closes an important Claude-style gap: `nanoclaw` no longer depends only on
working-memory snapshots plus manual promotion tools. It now has an append-only
capture surface that can support a future extraction or consolidation pipeline.

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

Structured session notes now also feed the operator's session catalog surface:

- when `.nanoclaw/memory/working/sessions/<session>.md` has a `Session Title`,
  `/sessions` prefers that title over the raw last-user-prompt preview
- `/agent_sessions` carries the parent session title so resume-oriented lists
  expose the same high-signal cue
- `/sessions <query>` now includes title-only matches from structured session
  notes even when the query text never appeared in the visible transcript
- those title matches are merged ahead of transcript-only hits, but this stays
  a host-side enrichment so the underlying session-store schema remains
  transcript-focused and memory-backend agnostic

That is closer to Claude's resume picker shape, where session-memory-derived
titles act as first-class selector metadata instead of staying trapped inside
continuity notes that only compaction and recall can see.

Those titles now also participate in host-side operator reference resolution:

- `/session`, `/export_session`, `/export_transcript`, `/agent_sessions <ref>`,
  and `/tasks <ref>` still prefer hard ids and prefixes first
- when no id or prefix matches, those commands fall back to a unique
  `Session Title` match from the structured session note
- `/resume <ref>` uses the same fallback, but resolves a unique matching
  session title to that session's root agent-session instead of treating
  worker agent ids as title-addressable selectors

That keeps the durable store schema transcript-centric while moving the
operator-facing selection semantics closer to Claude's session picker behavior.

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
