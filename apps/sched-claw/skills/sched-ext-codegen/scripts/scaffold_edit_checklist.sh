#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scaffold_edit_checklist.sh --output FILE --candidate-id ID
                             [--design-brief PATH]
                             [--source-target PATH]
                             [--touchpoint TEXT]...
                             [--hook NAME]...
                             [--map TEXT]...
                             [--dsq TEXT]...
                             [--guard TEXT]...
                             [--build-command TEXT]
                             [--verify-command TEXT]

Creates a durable edit checklist that narrows a sched-ext candidate change into
explicit code touchpoints, hook coverage, and rollout guards without pushing a
fixed workflow back into the host.
EOF
}

output_path=""
candidate_id=""
design_brief=""
source_target=""
build_command=""
verify_command=""
touchpoints=()
hooks=()
maps=()
dsqs=()
guards=()

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
    --candidate-id)
      candidate_id="${2:?missing value for --candidate-id}"
      shift 2
      ;;
    --design-brief)
      design_brief="${2:?missing value for --design-brief}"
      shift 2
      ;;
    --source-target)
      source_target="${2:?missing value for --source-target}"
      shift 2
      ;;
    --touchpoint)
      touchpoints+=("${2:?missing value for --touchpoint}")
      shift 2
      ;;
    --hook)
      hooks+=("${2:?missing value for --hook}")
      shift 2
      ;;
    --map)
      maps+=("${2:?missing value for --map}")
      shift 2
      ;;
    --dsq)
      dsqs+=("${2:?missing value for --dsq}")
      shift 2
      ;;
    --guard)
      guards+=("${2:?missing value for --guard}")
      shift 2
      ;;
    --build-command)
      build_command="${2:?missing value for --build-command}"
      shift 2
      ;;
    --verify-command)
      verify_command="${2:?missing value for --verify-command}"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$output_path" || -z "$candidate_id" ]]; then
  usage >&2
  exit 2
fi

mkdir -p "$(dirname "$output_path")"

render_task_list() {
  if (($# == 0)); then
    printf -- '- [ ] <fill me in>\n'
    return
  fi
  local item
  for item in "$@"; do
    printf -- '- [ ] %s\n' "$item"
  done
}

cat >"$output_path" <<EOF
# sched-ext edit checklist: ${candidate_id}

## Inputs
- candidate id: \`${candidate_id}\`
- design brief: \`${design_brief:-<fill me in>}\`
- source target: \`${source_target:-<fill me in>}\`

## Code touchpoints
$(render_task_list "${touchpoints[@]}")

## sched-ext hooks to inspect or edit
$(render_task_list "${hooks[@]}")

## Maps or per-entity state
$(render_task_list "${maps[@]}")

## DSQ or topology changes
$(render_task_list "${dsqs[@]}")

## Instrumentation and debug visibility
- [ ] confirm startup or fallback logs still distinguish this candidate from the last stable scheduler
- [ ] keep one narrow verification signal that proves the new policy path executed

## Build and verifier loop
- [ ] build command: \`${build_command:-<fill me in>}\`
- [ ] verifier command or next action: \`${verify_command:-<fill me in>}\`
- [ ] capture compiler stderr and verifier stderr next to the candidate artifacts

## Rollout guards
$(render_task_list "${guards[@]}")

## Exit criteria
- [ ] every touched hook, DSQ, or map still matches the design brief
- [ ] build and verifier output are archived
- [ ] rollout guards are explicit before activation
EOF
