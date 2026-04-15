---
name: codebase-inspection
description: Use when you need to understand existing architecture, call chains, ownership boundaries, or where a behavior actually lives before changing code.
aliases:
  - inspect-codebase
  - architecture-inspection
tags:
  - analysis
  - architecture
  - maintenance
---

Use this skill when the task is primarily about understanding the current codebase rather than immediately editing it.

Workflow:
1. Start from the user-facing behavior, command, file, or symptom instead of browsing randomly.
2. Retrieve the concrete entrypoints first: CLI parsing, top-level handlers, runtime boot, or exported APIs.
3. Trace the full path through the relevant modules, data structures, and side effects before drawing conclusions.
4. Treat implementation and executed tests as stronger evidence than comments or README text. Flag mismatches explicitly.
5. When multiple subsystems touch the behavior, separate facts from inferences and identify the exact ownership boundary.
6. Summarize findings with file references, observed behavior, and any unresolved gaps that still require runtime verification.

Do not propose broad refactors until the current behavior has been verified from code.
