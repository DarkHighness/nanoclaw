# Activation Checklist

## Before activate
- candidate build success
- verifier success
- daemon reachable
- rollback triggers written down

## During activate
- capture activation label
- inspect daemon logs immediately
- keep the workload window short and attributable

## After the window
- stop the candidate
- persist daemon logs
- compare against the baseline using the same metric surface
