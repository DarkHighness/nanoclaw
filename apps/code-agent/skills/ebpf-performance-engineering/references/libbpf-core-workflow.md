# libbpf CO-RE workflow

This workflow combines libbpf and eBPF CO-RE references with the practical build flow described in the Oracle Linux article on libbpf tracing.

## Standard file split
- `foo.bpf.c`: kernel-side eBPF program
- `foo.c` or similar: userspace loader
- shared headers for event structs or constants when needed

## Build outline

Generate `vmlinux.h` when required:

```bash
bpftool btf dump file /sys/kernel/btf/vmlinux format c > vmlinux.h
```

Compile the BPF object:

```bash
clang -g -O2 -target bpf -D__TARGET_ARCH_x86 -c foo.bpf.c -o foo.bpf.o
```

Optionally strip debug data for packaging:

```bash
llvm-strip -g foo.bpf.o
```

Generate the skeleton header:

```bash
bpftool gen skeleton foo.bpf.o > foo.skel.h
```

Compile the userspace loader and link libbpf:

```bash
cc -g -O2 foo.c -lbpf -lelf -lz -o foo
```

## Loader lifecycle

The generated skeleton exposes a standard lifecycle:
- `foo__open()`
- set globals or read-only config if needed
- `foo__load()`
- `foo__attach()`
- poll ring buffer or read maps
- `foo__destroy()` during cleanup

Use the skeleton instead of string-based map or program lookups when possible. It reduces loader drift from the BPF object layout.

## Validation checklist
- The program loads without verifier rejection.
- Expected events are observed.
- Output aligns with another signal source such as `perf`, `iostat`, or workload logs.
- Event rate and map sizes stay within the expected overhead budget.

## Cleanup
- Destroy links or pinned state explicitly.
- Remove temporary pinned objects under `/sys/fs/bpf` if the probe should not persist.
