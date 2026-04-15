# Performance report structure

This reference is synthesized from:
- Brendan Gregg, "Performance Analysis Methodology", https://www.brendangregg.com/methodology.html
- Brendan Gregg, "Active Benchmarking", https://www.brendangregg.com/activebenchmarking.html
- Brendan Gregg, "The USE Method: Linux", https://www.brendangregg.com/USEmethod/use-linux.html
- Brendan Gregg, "Linux Performance Analysis in 60,000 Milliseconds", https://www.brendangregg.com/Articles/Netflix_Linux_Perf_Analysis_60s.pdf

## Required sections

1. Problem statement
   - What regressed or failed?
   - When was it observed?
   - What business or user-facing symptom matters?

2. Environment and workload
   - Host or container scope
   - Kernel and hardware context
   - Input size, concurrency, request mix, benchmark mode, pinning

3. Evidence inventory
   - Exact commands
   - Time range
   - Artifact paths
   - Notes on missing privileges or capture limits

4. Findings
   - Facts
   - Inferences
   - Unknowns

5. Bottleneck ranking
   - Primary limiter
   - Secondary contributors
   - Rejected hypotheses and why they were rejected

6. Recommendations
   - Immediate mitigation
   - Next validating experiment
   - Longer-term engineering work

## Report quality rules
- Every important finding should cite the evidence path.
- State confidence explicitly when evidence is partial or conflicting.
- If USE, top-down, or CPI reasoning is used, explain why it supports the conclusion.
- For benchmark reports, always state the limiting factor, not just the score.
