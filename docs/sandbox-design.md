# Sandbox Design

This note defines how `nanoclaw` should model sandboxing as a first-class substrate concern instead of a `bash`-specific prompt convention.

The short version is:

- approval answers "may this action be attempted?"
- sandbox answers "what can this action technically touch even if it is approved?"
- host escape answers "who is allowed to widen that boundary, and through which control plane?"

Those are different questions and should remain different mechanisms.

## Problem Statement

The current repository already has two useful foundations:

- runtime-level approval policy and approval handling
- multi-root path policy for file-like tools

That is a good start, but it is not yet a sandbox.

Today, local process execution still bypasses any unified boundary:

- `bash` spawns child processes directly
- command hooks spawn `/bin/sh -lc` directly
- MCP `stdio` transports spawn local processes directly

That means the current substrate can ask for approval before some risky calls, but it does not yet provide one shared, enforced technical boundary for local execution surfaces.

## Goals

- keep sandboxing provider-agnostic and host-composed
- separate approval policy from sandbox policy
- enforce one shared boundary across all local process execution surfaces
- preserve the current minimal runtime and tool abstractions where possible
- support a low-friction default posture for code agents:
  - workspace write access
  - network off
  - protected sensitive paths inside otherwise writable roots
- make backend enforcement pluggable so hosts can choose OS-native or container-backed isolation
- fail closed when a host explicitly requires sandboxing and the backend is unavailable

## Non-Goals

- this note does not define a user-facing config file format for every host app
- this note does not require Docker as the default or only backend
- this note does not try to solve malicious kernel escapes or hostile local administrators
- this note does not make MCP annotations into hard security guarantees
- this note does not add a model-visible `elevated: true` escape hatch to normal tools

## Threat Model

The primary threats this design addresses are:

- accidental destructive local command execution
- prompt-injection-driven exfiltration after the model consumes open-world content
- local process launch surfaces silently bypassing the host's intended workspace boundary
- MCP `stdio` servers and hook commands running with broader host access than local tools
- sensitive path mutation inside the workspace, such as `.git` or future agent-private state

The design assumes:

- the host process itself is trusted
- a sandbox backend can only enforce what the underlying OS or container runtime supports
- remote HTTP services are a different trust boundary from local child processes and should not be conflated

## Design Constraints

### Approval and sandbox are different layers

Runtime approval already exists and should stay in the runtime control plane.

Sandboxing should not be implemented as:

- a special case inside `bash`
- a shell-only prompt convention
- a boolean annotation on tools
- transport config attached to MCP server definitions

Sandboxing is a host-injected execution boundary that local child processes inherit.

### Tool annotations stay advisory

`destructiveHint` and `openWorldHint` are still useful for default approval prompts, but they remain hints. They do not define filesystem, network, or host-escape rights.

### Transport config and security policy stay separate

`McpServerConfig` should continue to describe transport:

- stdio command, args, env, cwd
- streamable HTTP URL and headers

It should not become the place where sandbox policy lives. A host should be able to connect to the same transport using different local execution boundaries.

### The substrate is host-composed

The foundation should expose Rust abstractions and safe defaults. Concrete products may add:

- interactive approvals
- persistent allowlists
- enterprise policy injection
- platform-specific sandbox backends

Those are host responsibilities layered on top of the substrate contract.

## Control Model

The substrate should model three distinct controls.

### 1. Approval policy

Existing runtime concern.

Answers:

- should this tool call be auto-allowed?
- should it be denied?
- should the host review it?

Inputs:

- tool name
- tool origin
- selected argument fields
- advisory tool annotations
- host hooks

### 2. Sandbox policy

New execution-boundary concern.

Answers:

- what roots can a spawned process read?
- what roots can it write?
- which paths remain protected even inside writable roots?
- can it use the network?
- if yes, is network domain-scoped or full?
- is host escape completely denied, host-controlled, or unavailable?

Inputs:

- host-selected session policy
- execution origin
- workspace and auxiliary roots
- protected path defaults
- backend capability

### 3. Host escape control

Separate control-plane concern.

Answers:

- may the session widen its boundary?
- can a local process ever run outside the sandbox?
- if so, who approves that change?

Host escape is not normal tool capability. It is a runtime or host control decision.

## Proposed Abstractions

The shared execution boundary should live under `tools::process` so that:

- local tools can use it directly
- runtime hook executors can reuse it
- the `mcp` crate can reuse it without adding a new workspace crate

### `SandboxPolicy`

This is the typed policy the host injects.

```rust
pub struct SandboxPolicy {
    pub mode: SandboxMode,
    pub filesystem: FilesystemPolicy,
    pub network: NetworkPolicy,
    pub host_escape: HostEscapePolicy,
    pub fail_if_unavailable: bool,
}

pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

pub struct FilesystemPolicy {
    pub readable_roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub protected_paths: Vec<PathBuf>,
}

pub enum NetworkPolicy {
    Off,
    AllowDomains(Vec<String>),
    Full,
}

pub enum HostEscapePolicy {
    Deny,
    HostManaged,
}
```

Important constraints:

- `protected_paths` win over writable roots
- `readable_roots` and `writable_roots` are explicit runtime inputs, not inferred from annotations
- `DangerFullAccess` is still typed policy, not "skip the abstraction"

### `ExecutionOrigin`

The executor needs to know why a process is being started.

```rust
pub enum ExecutionOrigin {
    BashTool,
    HookCommand,
    McpStdioServer { server_name: String },
    HostUtility { name: String },
}
```

This is more specific than `ToolOrigin`. The runtime cares whether the process came from a hook or an MCP server even though both are "local" from the model's point of view.

### `ExecRequest`

All local child-process launches should normalize into one request shape.

```rust
pub struct ExecRequest {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub origin: ExecutionOrigin,
    pub sandbox_policy: SandboxPolicy,
    pub runtime_scope: RuntimeScope,
}

pub struct RuntimeScope {
    pub run_id: Option<RunId>,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<CallId>,
}
```

The runtime scope keeps auditing and denial logs attributable without teaching the executor about higher-level transcript internals.

### `ProcessExecutor`

All local process launch surfaces should reuse one trait.

```rust
#[async_trait]
pub trait ProcessExecutor: Send + Sync {
    async fn run(&self, request: ExecRequest) -> Result<ExecOutput>;
    async fn spawn(&self, request: ExecRequest) -> Result<Box<dyn ExecSession>>;
    async fn spawn_stdio(&self, request: ExecRequest) -> Result<StdioTransportHandle>;
}
```

This trait deliberately covers the three shapes already present in the repo:

- one-shot command execution
- long-running background command sessions
- stdio child-process transports

### `SandboxBackend`

`ProcessExecutor` is the stable substrate trait.

Concrete enforcement backends may sit behind it:

- `NoSandboxExecutor`
- `ManagedPolicyExecutor` for tests and hosts that only want explicit policy plumbing first
- `MacOsSeatbeltExecutor`
- `LinuxBubblewrapExecutor`
- `DockerExecutor`

The backend boundary stays hidden behind the executor so the runtime and tools do not learn platform-specific details.

The first enforcing implementation in this repository is a macOS Seatbelt-backed executor hidden behind `ManagedPolicyProcessExecutor`.

Important implementation constraints:

- it uses `sandbox-exec` with a generated profile rooted in `import "system.sb"` rather than trying to re-specify every dyld and system capability manually
- host paths must be canonicalized before they are embedded into the profile because Seatbelt matches real paths, not shell-friendly aliases such as `/var`
- unsupported platforms currently fall back to host execution unless `fail_if_unavailable` is set, so hosts can adopt typed policy wiring before every backend exists
- the first backend only inherits Apple system executable paths plus the host-selected roots, so user-space binaries outside those roots remain blocked unless the host widens policy or uses a permissive executor

## Context and Dependency Placement

### `ToolExecutionContext` stays small

`ToolExecutionContext` should remain about:

- workspace root
- worktree root
- optional sandbox root path mapping
- additional roots
- runtime ids

It should not absorb the full sandbox policy surface.

That keeps file tools focused on path translation and runtime scoping instead of becoming a generic security bag.

### Shared executor injection

The host should inject one `Arc<dyn ProcessExecutor>` and one session-level `SandboxPolicy` into the local execution surfaces it assembles:

- `BashTool`
- runtime command hooks
- MCP `stdio` connections

The exact wiring does not need to reuse a single context struct. What matters is that all three use the same executor contract and the same effective policy.

### `McpServerConfig` should stay transport-only

Instead of widening `McpServerConfig`, add a separate connect-time options object:

```rust
pub struct McpConnectOptions {
    pub process_executor: Arc<dyn ProcessExecutor>,
    pub sandbox_policy: SandboxPolicy,
}
```

This keeps MCP transport description separate from local execution rights.

## Default Policy

The recommended default for code-oriented hosts is:

- `SandboxMode::WorkspaceWrite`
- readable roots:
  - workspace root
  - worktree root when present
  - explicitly added auxiliary roots
- writable roots:
  - workspace root
  - worktree root when present
  - explicitly added writable roots
- protected paths:
  - `.git`
  - future agent-private state such as `.agent-core`
  - any host-private credential directories the host opts in
- `NetworkPolicy::Off`
- `HostEscapePolicy::Deny`

This matches the desired low-friction local coding posture:

- normal file edits work
- normal local build and test commands work if they do not require network
- accidental mutation of sensitive control paths stays blocked
- open-world exfiltration is substantially harder by default

Hosts can derive this posture directly from `ToolExecutionContext` when `workspace_only` is enabled. When a host explicitly disables workspace-only path policy, the current implementation keeps local child-process execution permissive instead of silently inventing a narrower boundary than the rest of the host surface advertises.

## Execution Surfaces

### Bash tool

`bash` should stop owning sandbox semantics.

It still owns:

- command session protocol
- output windowing
- timeout handling
- background session bookkeeping

It should no longer own:

- direct `Command::new`
- implicit trust of the local shell environment
- special-case path-only security logic pretending to be a sandbox

`bash` should build an `ExecRequest` and hand it to the shared executor.

### Command hooks

Command hooks are currently one of the easiest local-bypass paths because they run outside the tool registry but still execute arbitrary local commands.

They should use the same executor contract as `bash`, with a different `ExecutionOrigin`.

Hook policy may still differ from `bash` policy, but that difference should come from a host-supplied `SandboxPolicy`, not from a separate process-launch mechanism.

### MCP `stdio`

An MCP `stdio` server is a local child process with durable access to the session.

It should therefore inherit the same local execution boundary as other spawned processes.

The important consequence is that MCP `stdio` startup becomes a sandboxed execution path rather than a transport exception.

### MCP `streamable_http`

HTTP transport is a different boundary.

It does not need local child-process sandboxing, but it should still respect:

- host allow or deny rules for remote MCP servers
- network policy for outbound HTTP clients when the host chooses to unify those settings

This note does not require local process sandbox policy and remote HTTP allowlists to share the same implementation. They only need consistent host semantics.

## Approval, Denial, and Escalation

### Existing tool approval flow stays

Runtime tool approval still decides whether a tool call may proceed.

The existing order remains sound:

- hook gates
- runtime approval policy
- host approval handler

### Sandbox denial is not a tool approval

If a tool call is approved but the effective sandbox policy blocks the resulting local process, the executor should fail with a structured boundary error.

That error is then converted into an error tool result and fed back into the model loop, just like other local tool failures.

That separation matters:

- the tool call was approved
- the local process was still constrained

### No tool-level `elevated` parameter

The substrate should not add `elevated: true` to the `bash` tool or to MCP transport config.

If a host later wants interactive boundary widening, it should add a runtime-native control event or session-level permission mutation, not a normal tool argument.

That keeps "change the session trust boundary" separate from "run this ordinary command."

### Host escape roadmap

V1 should fail closed and surface clear errors when a command needs more access than the current policy allows.

If a later host wants interactive widening, it can add:

- a runtime-native "request boundary change" event
- host review and logging
- policy mutation at session scope

That is a better fit than teaching the model to toggle unsandboxed mode per tool call.

## Backend Strategy

### V1 backends

The first useful backends are:

- `NoSandboxExecutor`
  - preserves current behavior
  - useful for tests and incremental migration
- `EnforcingWorkspaceExecutor`
  - enforces roots, protected paths, and network off
  - may initially use host-supported primitives only where available

### Platform backends

After the substrate contract is stable, hosts can implement stronger backends:

- macOS seatbelt
- Linux bubblewrap plus seccomp
- Docker for container-oriented hosts

The framework should not hard-code one backend as the semantic definition of sandboxing.

### Fail-closed behavior

If `fail_if_unavailable` is false:

- the host may fall back to a weaker executor
- but it must emit an explicit warning and audit event

If `fail_if_unavailable` is true:

- startup or session creation should fail rather than silently widening execution rights

## Protected Paths

Protected paths are important enough to model explicitly instead of telling hosts to deny them with ad hoc approval rules.

Why they exist:

- `.git` is inside the workspace but should not be casually mutated
- future substrate-private state may also live inside the workspace
- path-based approval rules are too easy to drift from child-process reality

Rules:

- protected paths are always read-only from sandboxed child processes
- hosts may add more protected paths
- path protection should apply recursively

## Observability

The executor should emit structured events for:

- effective backend name
- effective sandbox mode
- writable and protected root summaries
- network mode
- denial reason
- backend unavailable fallback

The runtime already values auditability. Sandbox state should become part of that same contract.

## Migration Plan

### Phase 1: Extract the execution abstraction without changing behavior

- add `ExecRequest`, `ProcessExecutor`, and default `NoSandboxExecutor`
- route `bash` through the executor
- route command hooks through the executor
- route MCP `stdio` through the executor

Exit criterion:

- no remaining direct child-process launch path in the framework except inside executor implementations

### Phase 2: Add typed sandbox policy

- introduce `SandboxPolicy`, `FilesystemPolicy`, `NetworkPolicy`, and `ExecutionOrigin`
- pass session policy from hosts into `bash`, hooks, and MCP stdio startup
- keep `ToolExecutionContext` focused on roots and runtime ids

Exit criterion:

- local execution surfaces all receive the same typed policy even if the backend is still permissive

### Phase 3: Enforce workspace-write defaults

- enforce writable roots
- enforce protected paths
- enforce network off
- return structured denial errors

Exit criterion:

- `workspace_write + network_off + protected_paths` is real, not aspirational

### Phase 4: Add stronger backends

- add platform-specific or container-specific implementations behind the same trait
- support `fail_if_unavailable`

Exit criterion:

- at least one host can run with a real enforcing backend in normal development use

### Phase 5: Optional host-managed boundary escalation

- add runtime-native session policy change flow if a host needs it
- do not add tool-level escape flags

Exit criterion:

- boundary widening, if supported at all, is logged and host-mediated

## Test Strategy

### Unit tests

- policy normalization and precedence
- protected path precedence over writable roots
- domain allowlist matching
- `ExecutionOrigin` mapping and audit metadata

### Integration tests

- `bash` cannot write protected paths under workspace-write
- hook commands inherit the same write boundary as `bash`
- MCP `stdio` child processes inherit the same boundary as `bash`
- network-disabled sessions reject obvious outbound network use when backend support exists

### Regression tests

- no direct `Command::new` remains in `bash`, hooks, or MCP stdio connection setup outside executor implementations
- subagents inherit the same effective sandbox policy as the parent session unless the host explicitly overrides it

## Relationship To Existing Notes

- [design.md](/Users/twiliness/nanoclaw/docs/design.md) remains the architecture overview
- [tool-interface-design.md](/Users/twiliness/nanoclaw/docs/tool-interface-design.md) remains the contract note for local coding tools
- [tooling-research.md](/Users/twiliness/nanoclaw/docs/tooling-research.md) captures external research inputs that motivated the approval and multi-root path model

This note is narrower: it defines the local execution boundary and how that boundary should be composed into the substrate.
