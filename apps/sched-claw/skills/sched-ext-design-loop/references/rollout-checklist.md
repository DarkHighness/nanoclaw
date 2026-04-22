# sched-ext Rollout Checklist

## Before activation
- baseline evidence captured under CFS
- build succeeds on the target kernel/toolchain
- rollback criteria written down
- daemon status checked
- activation label chosen

## During activation
- inspect daemon logs immediately
- keep the verification window bounded
- run the exact workload used for the baseline
- capture any kernel debug dump or watchdog fallback

## After activation
- stop the scheduler explicitly, even if it appears healthy
- store final daemon status and exit code
- compare against baseline using the same metric definitions
- decide whether the result is promote, revise, or rollback
