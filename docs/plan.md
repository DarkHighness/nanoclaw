# Documentation Index

Date: 2026-03-28

Status: Active

## Current Entry Point

This file is the live documentation index after the 2026-03-28 implementation
pass and archive refresh.

Archived documentation now lives under:

- `docs/archive/README.md`
- `docs/archive/2026-03-28/README.md`
- `docs/archive/2026-03-28/2026-03-28-plugin-system-plan.md`
- `docs/archive/2026-03-28/2026-03-28-memory-system-plan.md`
- `docs/archive/2026-03-28/2026-03-28-multi-agent-plan.md`
- `docs/archive/2026-03-28/2026-03-28-model-config-plan.md`
- `docs/archive/2026-03-28/2026-03-28-remediation-checklist.md`

Earlier historical material remains available through the archive timeline.

## Current Repository State

As of 2026-03-28, the repository has already cleared the main remediation and
alignment work that the dated plans were created to drive.

Implemented and aligned:

- plugin system permission hardening, `HookEffect`, driver activation outcome
  host integration, wasm validation/runtime split, shared hook audit plane, and
  transcript mutation via `Current` / `MessageId` / `LastOfRole`
- memory runtime export production wiring, concurrent managed writes,
  `include_stale` filtering, working/coordination memory improvements, and
  corpus caching plus directory-scan short-circuiting
- multi-agent parent-child isolation, atomic batch spawn, `dependency_ids`
  scheduling, bounded child cold start, write-lease hardening, and session-map
  contention reduction
- model catalog / profile-based config, role-routed subagent profiles,
  profile-derived sandbox enforcement, `internal.memory` driver boot wiring,
  workspace `.env` / `.env.local` credential unification, capability metadata
  enforcement, and unified token usage ledger/UI

Intentionally still documented as not yet implemented:

- dedicated `prompt` / `agent` hook executors beyond fail-closed stubs
- any future modality-specific worker routing that would go beyond the current
  capability-enforcement surface

## How To Use Docs

- use this file as the root index
- use `docs/archive/README.md` as the archive timeline
- use `docs/archive/2026-03-28/README.md` for the full 2026-03-28 archive
  snapshot
- treat older dated archives as historical context, not as current status
  documents
- keep new design passes short-lived in `docs/` and archive them once the
  implementation slice is complete

## Live Documents Outside The Archive

- `docs/plan.md`
- `docs/sandbox-design.md`
  - current-state sandbox boundary note
- `docs/code-agent-delivery-plan.md`
  - active app-convergence and code-agent industrialization plan
- `docs/session-model-design.md`
  - canonical `Session` / `AgentSession` model and rename target for the
    repository-wide `Run*` migration
