---
name: regression-debugging
description: Use when something used to work and now fails, hangs, or behaves differently, and you need a disciplined regression investigation workflow.
aliases:
  - debug-regression
  - bug-triage
tags:
  - debugging
  - regression
  - testing
---

Use this skill when the task is to diagnose a regression, flaky behavior, or a newly introduced failure.

Workflow:
1. Reproduce the failure with the smallest reliable command, test, or scenario you can obtain.
2. Identify the exact failing boundary: input parsing, state transition, external process, persistence, rendering, or concurrency.
3. Compare the failing path with adjacent success paths and recent code changes before editing.
4. Prefer minimal fixes that restore the previous contract instead of refactoring during diagnosis.
5. Add or extend a targeted test that reproduces the regression whenever the harness supports it.
6. Re-run the focused verification after the fix and record what changed in behavior.

Avoid guessing root causes from symptoms alone; collect runtime evidence before committing to a fix.
