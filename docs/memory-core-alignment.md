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
base instructions bounded. This aligns the current host with Claude's
session-start behavior without introducing per-turn prompt recomputation yet.
