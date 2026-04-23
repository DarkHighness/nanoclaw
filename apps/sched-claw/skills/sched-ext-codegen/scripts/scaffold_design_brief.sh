#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scaffold_design_brief.sh --output FILE --candidate-id ID
                           [--source-target PATH]
                           [--evidence REF]...
                           [--analysis REF]...
                           [--lever TEXT]...
                           [--invariant TEXT]...
                           [--risk TEXT]...
                           [--fallback TEXT]...

Creates a Markdown design brief that bridges evidence and sched-ext code edits
without teaching the host one fixed workflow.
EOF
}

output_path=""
candidate_id=""
source_target=""
evidence_refs=()
analysis_refs=()
levers=()
invariants=()
risks=()
fallbacks=()

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
    --source-target)
      source_target="${2:?missing value for --source-target}"
      shift 2
      ;;
    --evidence)
      evidence_refs+=("${2:?missing value for --evidence}")
      shift 2
      ;;
    --analysis)
      analysis_refs+=("${2:?missing value for --analysis}")
      shift 2
      ;;
    --lever)
      levers+=("${2:?missing value for --lever}")
      shift 2
      ;;
    --invariant)
      invariants+=("${2:?missing value for --invariant}")
      shift 2
      ;;
    --risk)
      risks+=("${2:?missing value for --risk}")
      shift 2
      ;;
    --fallback)
      fallbacks+=("${2:?missing value for --fallback}")
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

render_list() {
  if (($# == 0)); then
    printf -- '- <fill me in>\n'
    return
  fi
  local item
  for item in "$@"; do
    printf -- '- %s\n' "$item"
  done
}

cat >"$output_path" <<EOF
# sched-ext design brief: ${candidate_id}

## Candidate
- id: \`${candidate_id}\`
- source target: \`${source_target:-<fill me in>}\`

## Evidence references
$(render_list "${evidence_refs[@]}")

## Analysis references
$(render_list "${analysis_refs[@]}")

## Policy levers
$(render_list "${levers[@]}")

## Invariants
$(render_list "${invariants[@]}")

## Risks
$(render_list "${risks[@]}")

## Fallback criteria
$(render_list "${fallbacks[@]}")

## Edit scope
- <fill in the concrete files, hooks, DSQs, or maps that will change>

## Notes
- Keep this brief aligned with the saved evidence and the current workload contract.
- Prefer the narrowest code change that can falsify the active scheduler hypothesis.
EOF
