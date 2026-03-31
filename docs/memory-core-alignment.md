# Memory Core Alignment

Date: 2026-03-31

Status: Active Design Note

## Goal

Align `memory-core` with the most important Claude-style memory capabilities
without changing the repository's existing workspace-local state model.

This slice intentionally aligns capability shape, not storage location:

- project instructions should be discoverable as long-lived procedural memory
- managed auto memory should expose one concise index file plus topic files
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
  - generated from managed memory so the backend has one compact entry point
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
- transient episodic, working, and coordination memory is summarized instead of
  being expanded in full

That keeps the file useful as a future session-primer input while avoiding a
second giant bucket of duplicated runtime text.

## Retrieval Implications

The generated auto-memory index is searchable, but `memory-core` treats it like
other summary files and slightly downranks it relative to the specific topic
file it points at. The index exists to help navigation and lightweight recall,
not to replace the source note.
