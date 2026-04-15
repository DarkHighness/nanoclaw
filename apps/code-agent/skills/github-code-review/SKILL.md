---
name: github-code-review
description: Use when reviewing a diff, branch, or PR and the main goal is to find correctness issues, regressions, risk, and missing coverage.
aliases:
  - code-review
  - pr-review
tags:
  - review
  - quality
  - risk
---

Use this skill when the user asks for a review or when you need a review-style pass over recent changes.

Workflow:
1. Read the diff first. Understand what changed before reading surrounding files in depth.
2. Prioritize findings over summaries. Look for correctness bugs, behavioral regressions, broken assumptions, unsafe migrations, and missing tests.
3. Verify claims against the current implementation and, when possible, existing tests or command output.
4. Order findings by severity and make each finding actionable: what is wrong, why it matters, and where it is.
5. Distinguish confirmed defects from speculative concerns. Mark hypotheses clearly.
6. Keep the closing summary brief and secondary to the findings.

If no material issues are found, say so explicitly and mention residual testing or confidence limits.
