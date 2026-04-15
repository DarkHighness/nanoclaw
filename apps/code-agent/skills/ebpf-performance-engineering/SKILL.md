---
name: "ebpf-performance-engineering"
description: "Use when a task requires writing, loading, or reasoning about eBPF programs for Linux performance analysis. Covers tool selection across bpftrace, BCC, and libbpf CO-RE, plus hook choice, build or load steps, map design, verification, and overhead control. Do not trigger for generic Linux perf analysis that can be handled without custom eBPF work."
---

# eBPF Performance Engineering

## When to use
- Write or adapt an eBPF probe for performance analysis.
- Decide whether `bpftrace`, BCC, or libbpf CO-RE is the right implementation path.
- Load, attach, and verify eBPF programs safely on Linux.

## Read before implementing
- `references/toolchain-selection.md` for choosing bpftrace, BCC, or libbpf CO-RE.
- `references/libbpf-core-workflow.md` for the standard compile, skeleton, load, attach, and cleanup flow.

## Workflow
1. Start from the measurement question.
   - What event, latency, queue, or stack attribution is missing?
   - What is the acceptable overhead?
2. Choose the right implementation level.
   - `bpftrace`: one-liners and short scripts for quick interactive diagnostics.
   - BCC: ready-made tools or Python-driven prototypes.
   - libbpf CO-RE: version-portable, reusable probes that belong in a repository or product workflow.
3. Choose the safest hook.
   - Prefer tracepoints or `fentry/fexit` when available.
   - Use `kprobe/kretprobe` only when stable higher-level hooks do not exist.
   - Use uprobes or USDT for user-space behavior.
4. Keep the kernel-side program minimal.
   - Filter early.
   - Aggregate in maps when possible.
   - Emit only the data needed for the question.
   - Avoid high-cardinality keys unless the workload and memory budget justify them.
5. Load and verify deliberately.
   - Confirm privileges, kernel support, BTF availability, and toolchain availability.
   - Compare eBPF output with `perf`, `sysstat`, or known workload events when possible.
6. Clean up after the run.
   - Detach links, remove pinned objects if used, and note any kernel settings changed for the run.

## Engineering rules
- Favor narrow probes over broad tracing.
- Prefer latency histograms, counters, and sampled attribution to verbose per-event logs.
- Measure or estimate overhead when event rates are high.
- Document the kernel assumptions and required capabilities.

## Output expectations
- State why eBPF is needed instead of `perf` or plain counters.
- Name the hook, map or buffer strategy, load path, and validation plan.
- If implementing code, include build and run instructions.
