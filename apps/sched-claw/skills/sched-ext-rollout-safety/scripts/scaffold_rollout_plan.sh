#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scaffold_rollout_plan.sh --output PATH --candidate LABEL
                           [--lease-seconds N]
                           [--activation-command TEXT]
                           [--stop-command TEXT]
                           [--rollback-trigger TEXT]
                           [--guardrail TEXT]

Examples:
  scaffold_rollout_plan.sh --output artifacts/rollout.md --candidate cand-a \
    --lease-seconds 30 \
    --activation-command "sched-claw daemon activate --lease-seconds 30 ..." \
    --rollback-trigger "p95 latency > baseline + 10%" \
    --guardrail "throughput must not regress"
EOF
}

output_path=""
candidate_label=""
lease_seconds=""
activation_command=""
stop_command=""
rollback_triggers=()
guardrails=()

while (($# > 0)); do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --output)
      output_path="${2:?missing value for --output}"
      shift 2
      ;;
    --candidate)
      candidate_label="${2:?missing value for --candidate}"
      shift 2
      ;;
    --lease-seconds)
      lease_seconds="${2:?missing value for --lease-seconds}"
      shift 2
      ;;
    --activation-command)
      activation_command="${2:?missing value for --activation-command}"
      shift 2
      ;;
    --stop-command)
      stop_command="${2:?missing value for --stop-command}"
      shift 2
      ;;
    --rollback-trigger)
      rollback_triggers+=("${2:?missing value for --rollback-trigger}")
      shift 2
      ;;
    --guardrail)
      guardrails+=("${2:?missing value for --guardrail}")
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$output_path" || -z "$candidate_label" ]]; then
  echo "--output and --candidate are required" >&2
  exit 2
fi

mkdir -p "$(dirname "$output_path")"

{
  printf '# rollout plan: %s\n\n' "$candidate_label"
  printf '## Activation\n'
  printf -- '- candidate: `%s`\n' "$candidate_label"
  if [[ -n "$lease_seconds" ]]; then
    printf -- '- lease: `%ss`\n' "$lease_seconds"
  else
    printf -- '- lease: `<fill>`\n'
  fi
  if [[ -n "$activation_command" ]]; then
    printf -- '- activation command: `%s`\n' "$activation_command"
  else
    printf -- '- activation command: `<fill>`\n'
  fi
  if [[ -n "$stop_command" ]]; then
    printf -- '- stop command: `%s`\n' "$stop_command"
  else
    printf -- '- stop command: `sched-claw daemon stop --graceful-timeout-ms 2000`\n'
  fi
  printf '\n## Guardrails\n'
  if ((${#guardrails[@]} == 0)); then
    printf -- '- <fill>\n'
  else
    for guardrail in "${guardrails[@]}"; do
      printf -- '- %s\n' "$guardrail"
    done
  fi
  printf '\n## Rollback Triggers\n'
  if ((${#rollback_triggers[@]} == 0)); then
    printf -- '- <fill>\n'
  else
    for trigger in "${rollback_triggers[@]}"; do
      printf -- '- %s\n' "$trigger"
    done
  fi
  printf '\n## Evidence Checklist\n'
  printf -- '- latest build success recorded\n'
  printf -- '- latest verifier result recorded\n'
  printf -- '- daemon status checked before activation\n'
  printf -- '- daemon logs captured immediately after activation\n'
  printf -- '- stop decision and final logs persisted\n'
} >"$output_path"
