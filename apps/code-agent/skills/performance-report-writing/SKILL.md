---
name: "performance-report-writing"
description: "Use when a task requires writing a Linux performance analysis report from perf, sysstat, benchmark, or eBPF evidence. Focus on evidence-backed findings, bottleneck ranking, explicit facts versus inferences, and clear next experiments. Do not trigger for generic prose editing unrelated to performance investigations."
---

# Performance Report Writing

## When to use
- Write a report after a Linux performance investigation.
- Turn benchmark evidence into an engineering report that names limiting factors.
- Produce an executive summary plus technical findings for stakeholders.

## Read before writing
- `references/report-structure.md` for the required sections and evidence rules.
- `assets/report-template.md` for a ready-to-fill report skeleton.

## Workflow
1. Define the report target.
   - Incident summary, benchmark result, regression analysis, or optimization proposal.
   - Name the audience: kernel engineer, service owner, manager, or mixed.
2. Build the report around evidence, not intuition.
   - Every important claim should reference a command, artifact, trace, or code location.
   - If a claim is inferential, label it as inference.
3. Separate three categories explicitly.
   - Fact: directly observed metrics, stack traces, histograms, or logs.
   - Inference: conclusion drawn from those facts.
   - Unknown: missing evidence, unresolved alternative explanation, or data quality limitation.
4. Name the limiting factor.
   - If the report covers a benchmark, state the current limiter and why it dominates.
   - If evidence is insufficient to name a limiter, say so plainly and propose the next capture.
5. End with concrete next actions.
   - Immediate mitigations, validating experiments, code changes, or infrastructure checks.

## Writing rules
- Do not present a benchmark number without workload, environment, and limiter context.
- Include exact commands or artifact paths for important evidence.
- Say when conclusions are specific to one workload, kernel, or hardware configuration.
- Keep recommendations ranked by impact and confidence.

## Output expectations
- Default output path: `performance_report.md` unless the user requested another location.
- The report should be readable by someone who did not watch the investigation happen live.
