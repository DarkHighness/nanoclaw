# Code Agent Approval Policy Plan

Date: 2026-04-10

Status: Completed

Follow-up note: `write_stdin` no longer carries its own approval burden. The
harmful decision belongs to `exec_command`, so stdin follow-ups stay approval
free while `exec_command` keeps the host-scoped approval boundary.

Follow-up note: `update_plan` and `update_execution` now stay approval-free as
well. They mutate host-owned coordination state, not the workspace or an
external system, so they no longer share the filesystem or process approval
path.

Follow-up note: MCP resource reads are now transport-aware. `code-agent`
auto-allows `read_mcp_resource` only when the request resolves to a locally
launched `stdio` MCP server. Remote `streamable_http` MCP resources still stay
on the default approval path.

## Goal

Reduce repeated approval prompts in `apps/code-agent` for a narrow set of safe,
code-owned read-only tools without weakening the substrate-wide approval model.

This plan intentionally keeps the change host-scoped.

## Problem Statement

`crates/runtime/src/runtime/tool_flow.rs` derives approval prompts from
`ToolApprovalProfile` flags such as `mutates_state`, `needs_network`,
`open_world`, and `needs_host_escape`.

That baseline logic does **not** treat `read_only` as an automatic allow.

`apps/code-agent` currently wires a `ToolApprovalHandler`, but it does not
install a host `ToolApprovalPolicy` for either the root runtime or the subagent
runtime bootstrap in `apps/code-agent/src/backend/boot.rs`.

The result is predictable but too blunt for the product surface:

- clearly local read-only tools such as `read`, `glob`, `grep`, and code-intel
  helpers already avoid approval because they do not advertise network,
  open-world, host-escape, or mutation behavior
- code-owned read-only remote discovery tools such as `web_search` and
  `web_fetch` still trigger approval because they advertise `needs_network` and
  `open_world`
- `/permissions` does not affect that behavior because it only swaps the base
  sandbox policy for the session; it is not an approval-policy control plane

## Constraints

- do not change substrate-wide approval semantics just to tune `code-agent`
- do not blanket-allow every tool with `read_only = true`
- do not treat MCP or custom-tool `read_only` declarations as trustworthy enough
  for a global or generic host allow rule
- keep root-runtime and subagent behavior aligned

## Proposed Host Policy

Add a code-agent-owned `ToolApprovalRuleSet` that auto-allows only a narrow
allowlist of host-owned safe read-only tools.

First implementation slice:

- allow `web_search`
- allow `web_fetch`

Those tools are both code-owned, read-only, and already constrained by the
host's normal HTTP policy layer. Auto-allowing them removes repeated review for
common research flows without trusting arbitrary external tool metadata.

Explicitly **not** included in this slice:

- broad `exec_command` trust widening
- mutating filesystem tools
- MCP tool calls in general
- custom tools in general
- remote `read_mcp_resource`, because its safety envelope still depends on the
  connected MCP server rather than a code-agent-owned HTTP surface

## Implementation Plan

1. Add a backend-local approval-policy helper module in `apps/code-agent`.
2. Build a first-match allow rule for exact local tool names `web_search` and
   `web_fetch`.
3. Install the same policy into:
   - the root `AgentRuntimeBuilder`
   - the `RuntimeSubagentExecutor`
4. Keep runtime baseline approval logic unchanged so other hosts preserve the
   existing contract.
5. Update product-facing docs to clarify that:
   - approval policy and sandbox policy are separate
   - `/permissions` only changes the base sandbox mode
   - code-agent now auto-allows a narrow built-in read-only web-research slice

## Completed Scope

- `apps/code-agent` keeps the approval relaxation host-scoped through
  `tool_approval_policy.rs` and the existing runtime wiring.
- `write_stdin`, `update_plan`, and `update_execution` stay approval-free
  because they continue an already-approved session or mutate host-owned
  coordination state.
- MCP tool and resource specs now carry transport-aware boundary metadata so
  approval rules can distinguish local `stdio` servers from remote
  `streamable_http` services.
- `read_mcp_resource` is auto-allowed only when the resolved MCP server boundary
  is a local process.
- product and design docs now describe the boundary split between sandbox mode,
  approval policy, and transport-aware MCP surfaces.

## Validation

- `cargo test --manifest-path crates/Cargo.toml -p types`
- `cargo test --manifest-path crates/Cargo.toml -p runtime approval`
- `cargo test --manifest-path crates/Cargo.toml -p mcp stdio_integration`
- `cargo test --manifest-path apps/Cargo.toml -p code-agent`

## Rollback Boundary

If the allowlist proves too permissive, roll back only the code-agent host
policy layer. The substrate runtime approval contract should remain unchanged.
