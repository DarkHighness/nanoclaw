# Choosing the eBPF toolchain

This reference is grounded in:
- bpftrace official site, https://bpftrace.org/
- bpftrace repository, https://github.com/bpftrace/bpftrace
- libbpf overview, https://libbpf.readthedocs.io/en/latest/libbpf_overview.html
- eBPF CO-RE docs, https://docs.ebpf.io/concepts/core/
- Oracle Linux blog, "Writing a block I/O filter using libbpf and eBPF tracing framework", https://blogs.oracle.com/linux/post/writing-a-block-io-filter-using-libbpf-and-ebpf-tracing-framework

## Tool choice

Use `bpftrace` when:
- You need an interactive one-liner or short script.
- The question is exploratory.
- Existing `bpftrace` tools already fit the problem.

Use BCC when:
- A packaged tool already exists and answers the question.
- A Python-driven prototype is good enough.
- The environment already ships BCC operationally.

Use libbpf CO-RE when:
- The probe should be checked into a repository.
- You need a reusable loader and controlled integration surface.
- Cross-kernel portability matters.
- You need tighter control over maps, ring buffers, and lifecycle.

## Hook preference

Prefer this order when possible:
1. Tracepoints
2. `fentry/fexit`
3. Uprobes or USDT for user-space
4. `kprobe/kretprobe`

Higher-level and typed hooks usually survive kernel changes better than raw probes.

## Overhead controls
- Filter in kernel, not in userspace.
- Aggregate in maps when possible.
- Avoid printing every event on hot paths.
- Use histograms or periodic summaries instead of per-event logs.
- If event volume is extreme, sample or narrow by PID, cgroup, CPU, or device.

## Environment checks
- Confirm root or the needed capabilities on the target system.
- Confirm `clang`, `bpftool`, and `libbpf` availability for CO-RE workflows.
- Confirm BTF availability when relying on CO-RE or `fentry/fexit`.
