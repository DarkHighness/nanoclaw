# Session Model Design

Date: 2026-03-29

Status: Active Design Note

This note defines the canonical `Session` and `AgentSession` model for the
repository-wide rename away from the overloaded `Run*` terminology.

The goal is not a cosmetic rename. The goal is to make persisted history,
runtime execution windows, and operator-facing resume semantics line up with the
actual product model.

## Why This Note Exists

The current substrate uses two ids:

- `RunId`
- `SessionId`

Those names no longer match the product semantics the host is converging on.

The host already exposes the top-level interaction as a `session`, while the
runtime and store still speak in terms of `run`. That mismatch created a false
rename path where the old `SessionId` was treated as the primary operator
session identifier.

That is not the target model.

## Canonical Vocabulary

There are two different session layers.

### Session

A `Session` is the full persisted conversation history for one multi-turn
dialogue with the provider.

It is the canonical top-level history object.

Its identifier is:

- `SessionId`

It owns:

- the append-only conversation transcript
- provider-facing history continuity
- durable history browsing and export
- the canonical event log for the dialogue

A `Session` may outlive multiple runtime windows and multiple agent executions.

### AgentSession

An `AgentSession` is one agent's runtime use of a `Session`.

Its identifier is:

- `AgentSessionId`

It is bound to exactly one `SessionId` at a time.

It owns:

- one root-agent or subagent runtime lifetime
- the effective request window derived from the bound `Session`
- runtime-local state such as continuation, compaction boundary, and token
  ledger snapshots for that execution window
- resume targets for operator workflows

An `AgentSession` ends when the host intentionally rotates the runtime window,
for example after a compaction boundary that should produce a new execution
context, or after a true backend reset/new-session action.

### Turn

`TurnId` remains the per-request / per-response unit inside one `AgentSession`.

It is not a durable session identifier and should not be exposed as a resume
target.

## Required Invariants

- Every `AgentSession` belongs to exactly one `Session`.
- A `Session` can have many `AgentSession`s over time.
- A subagent always gets its own `AgentSessionId`.
- A root agent and its subagents may share one `SessionId`, but they must never
  share the same `AgentSessionId`.
- Operator-facing `resume` targets an `AgentSessionId`, not a top-level
  `SessionId`.
- Durable transcript ownership belongs to `Session`, not to `AgentSession`.

## Current Code Mapping

The current substrate concepts map most closely to the target model like this:

- current `RunId` is semantically closest to target `SessionId`
- current `SessionId` is semantically closest to target `AgentSessionId`

That is why a correct rename cannot start by blindly renaming the old
`SessionId` in isolation. The repository has to migrate both layers with the
final model in mind.

## Lifecycle Model

### Session Lifecycle

A new `SessionId` is created when the operator starts a new top-level
conversation.

Examples:

- host startup with no prior resume target
- explicit `/new`
- creating a fresh session from a forked workspace or different conversation
  root

The `SessionId` remains stable across normal multi-turn interaction and history
growth.

### AgentSession Lifecycle

A new root `AgentSessionId` is created when the host creates a new live runtime
window over a `Session`.

A new subagent `AgentSessionId` is created whenever a child agent starts.

A root `AgentSessionId` should be rotated when the host intentionally replaces
the effective runtime window rather than merely appending more turns to the same
window.

Examples:

- a compaction boundary that should create a new execution context
- an explicit backend reset
- an explicit new-agent-session action over an existing `Session`

The boundary is history-based, not turn-based. If automatic compaction happens
mid-turn, the prompt may remain on the pre-compaction `AgentSession` while the
rebuilt provider request and response continue on the post-compaction
`AgentSession`.

In the host UX, `/clear` and `/new` should be modeled as the same backend-owned
operation: start a fresh top-level `Session` while leaving prior sessions
durably traceable. They are session-rotation commands, not presentation-only
screen clears. A frontend-neutral session-operation contract should own that
workflow and return the refreshed startup/session snapshot needed by any
frontend to redraw itself coherently.

## Resume Semantics

The operator expectation for `/resume` is:

- select an `AgentSessionId`
- reconstruct or reattach the corresponding runtime window
- continue against the bound `SessionId`

That resume path should tolerate historical replay boundaries:

- when persisted compaction checkpoints exist, the host can reconstruct the
  visible transcript window and reattach it into a fresh live `AgentSession`
- when older compacted history lacks those checkpoints, the target remains
  history-only and must not pretend to be safely resumable

The operator expectation for browsing durable history is different:

- browse `Session` history
- inspect transcript and exports
- optionally resolve which `AgentSession`s were created inside that `Session`

That means the host needs both views:

- `Session` catalog
- `AgentSession` catalog

Those catalogs are related, but they are not interchangeable.

## Storage Model

The repository should persist both layers, but it should not duplicate the full
transcript in two canonical places.

### Canonical Session History

Persist the top-level dialogue history as the canonical append-only log:

- `sessions/<session-id>.jsonl`

This store owns:

- transcript-bearing events
- provider continuity-bearing events
- durable export and replay

### AgentSession History

Persist `AgentSession` history as a separate index or log that points into the
canonical session history:

- `agent-sessions/<agent-session-id>.json`
- or `agent-sessions/<agent-session-id>.jsonl`

This layer should record:

- `agent_session_id`
- `session_id`
- `parent_agent_session_id`
- `role`
- `started_at`
- `ended_at`
- `end_reason`
- boundary references into the canonical session history
- resumability metadata

This model satisfies both product requirements:

- keep Codex/Claude-style durable session logs
- keep agent-session history without forking the transcript into a second
  source of truth

## Naming Targets

The repo-wide target names are:

- `RunId` -> `SessionId`
- `RunStore` -> `SessionStore`
- `RunSummary` -> `SessionSummary`
- `RunSearchResult` -> `SessionSearchResult`
- `RunEventEnvelope` -> `SessionEventEnvelope`
- `RunEventKind` -> `SessionEventKind`
- current `SessionId` -> `AgentSessionId`

Fields and methods should follow the same split:

- `run_id` -> `session_id`
- current instance-level `session_id` -> `agent_session_id`
- `session_ids()` when it means agent runtimes -> `agent_session_ids()`

## Migration Order

The migration should happen in this order:

1. add this canonical model and stop ad hoc renames
2. rename the old runtime-instance `SessionId` layer to `AgentSessionId`
3. rename the old top-level `Run*` layer to `Session*`
4. introduce explicit `AgentSession` lifecycle boundaries for compaction/reset
5. switch host resume and catalogs to `AgentSessionId`

This order keeps the semantic conflict visible and prevents the repository from
ending up with a partially renamed but still conceptually wrong model.
