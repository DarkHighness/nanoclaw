# 2026-03-26 Documentation Archive

Date: 2026-03-26

## Scope

This archive captures the first large design pass for the repository after the
substrate became bootable and the host/runtime split had started to stabilize.

These notes are design targets and research references, not implementation
guarantees.

## Archived Documents

- `docs/archive/2026-03-26/design.md`
- `docs/archive/2026-03-26/plan.md`
- `docs/archive/2026-03-26/memory-plugin-design.md`
- `docs/archive/2026-03-26/plugin-system-design.md`
- `docs/archive/2026-03-26/sandbox-design.md`
- `docs/archive/2026-03-26/tool-interface-design.md`
- `docs/archive/2026-03-26/tooling-industrial-alignment.md`
- `docs/archive/2026-03-26/tooling-research.md`

## Snapshot Summary

At the time of this archive, the repository was defining:

- the substrate layering between runtime, tools, providers, skills, and hosts
- the local tool contract model for grounded coding workflows
- the first plugin, memory, and sandbox design targets
- the external industrial and research baselines used to shape those designs

## How To Read This Archive

- start with `design.md` for the architectural frame
- read `plan.md` for the staged implementation intent at that point in time
- use the subsystem notes as historical design references
- treat later archives as the authoritative record of what actually shipped
