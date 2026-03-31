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

## Compaction Snapshots

Claude-style memory continuity is not only about durable recall. It also relies
on a compacted session keeping a usable working-memory handoff for the next
turns and for later resume.

`nanoclaw` now persists that handoff as working memory:

- when conversation compaction completes, the host writes the latest compact
  summary into `.nanoclaw/memory/working/agent-sessions/<agent-session>.md`
- that write replaces the previous continuation snapshot for the same target so
  the file stays a bounded latest-state handoff instead of an append-only log
- the record is tagged as a continuation snapshot for the current runtime
  session
- later query-time recall checks this working scope before consulting durable
  procedural and semantic memories

That keeps post-compaction continuity in the memory system itself instead of
depending only on one synthetic transcript summary message.
