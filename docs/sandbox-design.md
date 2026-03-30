# Sandbox Design

Date: 2026-03-28

Status: Live Design Note

This note records the current sandbox boundary after the 2026-03-28
implementation pass.

For the earlier design-target version, see
`docs/archive/2026-03-26/sandbox-design.md`.

`crates/sandbox` is the canonical sandbox boundary for `nanoclaw`.

The short version is:

- approval answers "may this action be attempted?"
- sandbox answers "what can this action technically touch even if it is approved?"
- host escape answers "who can widen that boundary, and through which control plane?"
- `request_permissions` grants extra read, write, or network access on top of
  the current base sandbox for the rest of the turn or session
- `/permissions` changes the session base sandbox mode itself (`default` vs
  `danger-full-access`)

Those mechanisms solve different problems and stay separate.

## Current Boundary

The sandbox is no longer modeled as a `bash` implementation detail under
`tools::process`.

- `crates/sandbox` owns the policy model, backend selection, platform-specific
  command preparation, and allow-domains proxy runtime.
- `crates/tools` consumes `sandbox` for process tools such as `bash`.
- `tools::process` remains only as a compatibility facade so existing imports do
  not have to migrate atomically.
- model-visible file tools also consume `sandbox` path checks, so `.git`,
  `.nanoclaw`, and other protected subpaths are governed by the same top-level
  policy instead of ad hoc tool-local rules.

That dependency direction matters because read, write, patch, MCP stdio
processes, hook commands, and host utilities can all depend on the same
execution-boundary model without routing through tool-specific error or context
types.

## Module Structure

The crate is intentionally split by responsibility:

- `policy`: typed sandbox policy, execution request shape, and path-policy
  normalization helpers
- `manager`: backend availability checks, policy support checks, fallback
  decisions, and managed allow-domains proxy bootstrap
- `platform::linux`: Linux backend wiring (`bubblewrap`, seccomp integration,
  allow-domains bridge helpers)
- `platform::macos`: macOS Seatbelt profile generation and command wrapping
- `network_proxy`: host-side allow-domains SOCKS5 proxy runtime

This mirrors the current Codex public structure more closely than the older
`tools::process` layout: platform-independent policy and manager logic stay
separate from Linux/macOS backend rendering.

## Policy Model

The host injects a typed `SandboxPolicy` into each local child-process launch.

- filesystem policy defines readable roots, writable roots, and protected paths
- network policy defines `Off`, `AllowDomains`, or `Full`
- host escape policy defines whether a sandboxed request may widen its own
  boundary
- `fail_if_unavailable` decides whether unsupported enforcement is a hard error
  or a host fallback

`SandboxScope` is the host/tool-facing input used to derive recommended policies
from workspace layout. This keeps the sandbox API independent from
`ToolExecutionContext`.

## Session Controls

The host now exposes two distinct permission control planes:

- `request_permissions` is model-visible workflow state. It widens the current
  effective sandbox additively and can be granted for a turn or the whole
  session.
- `/permissions` is operator-visible session state. It swaps the base sandbox
  mode between the configured default policy and `danger-full-access`.

That split matters because the operator may intentionally widen the whole
session even when the model never requested a narrower additive grant, and a
model-granted permission should not silently rewrite the host's configured base
mode.

## Platform Backends

Linux currently uses `bubblewrap` as the enforcing backend, with seccomp filters
attached to the prepared command. Availability is probed by actually attempting
to create a minimal unprivileged `bwrap` sandbox, because binary presence alone
is not a reliable signal.

macOS uses `sandbox-exec` with generated Seatbelt profiles. The same high-level
policy model is rendered into SBPL instead of mount rules.

`AllowDomains` remains an enforcing policy only. If a compatible backend is not
available, the request fails instead of silently degrading to unrestricted host
network access.

## Consumer Guidance

New code should depend on `sandbox` directly when it needs sandbox policy or
process-boundary behavior.

`tools::process` should only own tool-specific process behavior such as the
interactive `bash` session protocol. It should not grow platform sandbox logic
again.

Likewise, filesystem tools should not open-code their own protected-path rules.
They should ask `sandbox` whether a path is readable or writable under the
effective policy, so process sandboxing and file-tool sandboxing stay aligned.
