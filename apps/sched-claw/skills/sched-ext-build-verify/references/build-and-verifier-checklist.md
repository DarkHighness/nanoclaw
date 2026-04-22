# Build and Verifier Checklist

## Build path
- confirm `source_path`
- confirm `object_path`
- confirm `build_command`
- run `sched-claw experiment build ...`

## Read the result in this order
1. compiler exit status
2. compiler stderr summary
3. verifier exit status
4. verifier stderr summary

## Typical verifier buckets
- missing BTF or helper availability
- invalid pointer lifetime or bounds
- map definition mismatch
- kernel-version-sensitive sched-ext assumptions
