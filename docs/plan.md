# Implementation Review Plan

Date: 2026-03-26

## Status

This file replaces the earlier design-pass notes after a repository-wide review of
`docs/` against the current implementation.

Current subsystem remediation for the 2026-03-28 plugin / memory / multi-agent
review now lives in:

- `docs/2026-03-28-remediation-checklist.md`

As of 2026-03-28, the remediation pass has already cleared:

- multi-agent `agent_wait` correctness, parent scope isolation, batch spawn
  atomicity, and `dependency_ids` scheduling
- memory `subagent/task` export production wiring, concurrent managed writes,
  `include_stale` semantics, and agent/task context bridging
- plugin hook permission hardening, fail-closed stub handlers, and
  `DriverActivationOutcome` host integration in both host apps

Archived documents now live under:

- `docs/archive/2026-03-26/`

That archive preserves the earlier architecture, tooling, sandbox, plugin, and
memory design notes as historical context. This file is the current working plan.

## Review Basis

The findings below are based on:

- reading every document that previously lived under `docs/`
- checking the corresponding implementations in `crates/` and `apps/`
- running workspace tests for both `crates/` and `apps/`

## What Is Implemented

The main substrate direction described by the earlier docs is already real code,
not just design intent.

Implemented and materially working:

- config-driven host boot, provider selection, and runtime assembly
- append-only transcript flow, compaction, and provider continuation handling
- persistent run storage with JSONL transcripts plus `runs.index.json`
- plugin discovery, validation, enablement, slot selection, and activation planning
- builtin memory-slot activation for `memory-core` and `memory-embed`
- `skill.toml` precedence with YAML frontmatter fallback
- shared sandbox policy and process-executor wiring across `bash`, command hooks,
  and MCP `stdio`
- structured file-tool contracts for `read`, `write`, `edit`, `patch`, `list`,
  `glob`, and `grep`
- structured outputs for many local tools, MCP tool `output_schema` propagation,
  and typed tool lifecycle events for hosts
- web tooling with redirect validation, DOM-based extraction, backend registry,
  and backend inspection
- `memory-embed` hybrid retrieval with vector-store backends, query expansion,
  reranking, MMR, runtime exports, and lifecycle manifests

## Implemented But Not Aligned With The Original Plan

### 1. `memory-core` is a simplified implementation

The old memory design targeted a local SQLite FTS5 / BM25 sidecar for
`memory-core`.

Current behavior is simpler:

- corpus is loaded on demand
- chunks are created in memory
- lexical scoring is computed in process
- there is no dedicated lexical SQLite index for `memory-core`

This is a simplification, not an optimization. It keeps the default backend
small and deterministic, but it does not meet the earlier “local index” design
target.

### 2. Plugin driver activation is host-coded, not registry-driven

The old plugin design proposed a small compiled driver registry with
`PluginDriverFactory`.

Current behavior is narrower:

- plugin discovery and activation planning are generic
- driver activation in host boot is a `match` over known builtin driver ids

This is also a simplification. It is acceptable while `memory-core` and
`memory-embed` are the only driver-backed plugins, but it is not the generic
registry described in the design note.

### 3. Sandbox abstraction is narrower than planned

The old sandbox design proposed a trait that directly modeled multiple process
shapes (`run`, `spawn`, `spawn_stdio`).

Current behavior centralizes policy and process construction through
`ExecRequest`, but the executor surface is:

- `prepare(ExecRequest) -> Command`

The behavior is aligned enough for shared enforcement, but the abstraction is a
simpler substrate than the original design text proposed.

### 4. Legacy config compatibility is merged directly, not materialized as a synthetic plugin

The old plugin compatibility strategy said top-level `skill_roots` and
`mcp_servers` should be converted into an implicit synthetic plugin.

Current behavior preserves compatibility by direct merge during host boot:

- config skill roots are merged with `plugin_plan.skill_roots`
- config MCP servers are extended with `plugin_plan.mcp_servers`

This is functionally compatible, but the control-plane representation is simpler
than the documented target.

## Documents That Became Stale

The following archived notes no longer describe the current repository state
accurately and should be treated as historical references, not current status
documents:

- `tool-interface-design.md`
- `tooling-industrial-alignment.md`

The main drift is that these notes still describe several items as “not yet
implemented” even though the code now has them:

- `ToolSpec.output_schema`
- `ToolResult.structured_content`
- MCP `output_schema` and structured tool-result propagation
- typed host-facing tool lifecycle events
- redirect-safe web policy
- DOM-based web extraction
- pluggable web-search backend selection and backend inspection

The provider edge still degrades rich tool results to text for providers that
need text-only transport, but even there the implementation is stronger than the
old docs claimed because the downgrade path now wraps rich data in a stable JSON
envelope instead of flattening everything to prose.

## Test Findings

Two tests currently fail, and both failures are around sandbox expectations that
appear to have drifted after implementation changes:

### `crates/`

- `sandbox::manager::tests::sandbox_backend_status_reports_unavailable_when_restrictive_policy_has_no_backend`

The test assumes default empty backend availability must always produce
`Unavailable`. The implementation now performs platform probing and may resolve a
real backend dynamically.

### `apps/`

- `reference_tui::boot::tests::bootstraps_runtime_from_configured_workspace`

The test expects the sidebar text to include
`sandbox: workspace-write, network off, best effort`, but the current summary
string now renders either:

- `enforced via <backend>`
- `backend required but unavailable (...)`
- `best effort host fallback (...)`

This looks like test drift, not a core feature regression.

## Optimization Opportunities

### Priority 0

- update repository docs so current status markers match shipped behavior
- fix the two sandbox-related test failures so CI matches the real behavior

### Priority 1

- decide whether `memory-core` should stay intentionally lightweight or be
  brought up to the original SQLite FTS5 / BM25 design target
- if the lightweight design is intentional, update memory docs to say so
  explicitly

### Priority 2

- move plugin driver activation from host-level `match` statements to a real
  compiled driver registry if more driver-backed plugins are expected
- continue improving provider-native structured tool-result transport so fewer
  providers need the JSON-envelope text fallback

### Priority 3

- add per-hit provenance improvements where useful, especially in memory and web
  retrieval paths
- keep the active plan short and operational, and archive future design passes
  instead of letting status notes accumulate contradictory state

## Immediate Next Steps

1. Refresh tests and docs around sandbox status and startup summary wording.
2. Rewrite the archived tooling-status notes into current-state documentation.
3. Make an explicit architectural call on `memory-core`:
   lightweight lexical backend vs SQLite FTS5 lexical index.
4. Revisit plugin driver activation only if the repository adds more compiled
   plugin kinds than the current memory slot.

## 2026-03-28 Follow-up

The active remediation order for the newer subsystem work is:

1. fix multi-agent correctness and parent-child isolation
2. fix memory production export and concurrent record safety
3. tighten plugin effect permissions and driver-outcome wiring
4. only then continue with performance and feature expansion
