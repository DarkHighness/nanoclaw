# Linux Scheduler Triage References

Use these documents as primary references when the task needs kernel-grounded interpretation rather than ad hoc folklore.

## Official kernel docs
- `sched_ext`: https://docs.kernel.org/scheduler/sched-ext.html
- CFS design: https://docs.kernel.org/6.6/scheduler/sched-design-CFS.html
- scheduler statistics: https://docs.kernel.org/6.16/scheduler/sched-stats.html
- PSI: https://docs.kernel.org/6.10/accounting/psi.html
- delay accounting: https://docs.kernel.org/accounting/delay-accounting.html
- ftrace: https://docs.kernel.org/6.4/trace/ftrace.html

## Man pages
- `perf sched`: https://man7.org/linux/man-pages/man1/perf-sched.1.html

## How to use them
1. Use the CFS and sched-ext docs to understand what the baseline scheduler is doing and what the replacement path can control.
2. Use PSI and sched-stats to interpret queueing and runnable-delay evidence before assuming the issue is wakeup placement alone.
3. Use `perf sched` and `ftrace` references only after low-overhead counters have narrowed the question.
