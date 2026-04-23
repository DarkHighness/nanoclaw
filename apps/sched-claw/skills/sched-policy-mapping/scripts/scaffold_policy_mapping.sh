#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scaffold_policy_mapping.sh --output FILE --objective TEXT
                             [--evidence REF]...
                             [--analysis REF]...
                             [--lever TEXT]...
                             [--invariant TEXT]...
                             [--question TEXT]...
                             [--invalidate TEXT]...

Creates a reusable scheduler policy mapping note that stays implementation-
agnostic until a candidate path is chosen.
EOF
}

output_path=""
objective=""
evidence_refs=()
analysis_refs=()
levers=()
invariants=()
questions=()
invalidations=()

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
    --objective)
      objective="${2:?missing value for --objective}"
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
    --question)
      questions+=("${2:?missing value for --question}")
      shift 2
      ;;
    --invalidate)
      invalidations+=("${2:?missing value for --invalidate}")
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$output_path" || -z "$objective" ]]; then
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
# scheduler policy mapping

## Objective
- ${objective}

## Evidence references
$(render_list "${evidence_refs[@]}")

## Analysis references
$(render_list "${analysis_refs[@]}")

## Candidate levers
$(render_list "${levers[@]}")

## Invariants
$(render_list "${invariants[@]}")

## Open measurement questions
$(render_list "${questions[@]}")

## Invalidation conditions
$(render_list "${invalidations[@]}")

## Notes
- Keep this note implementation-agnostic until a specific candidate path is chosen.
- Narrow the next code change to the smallest lever set that can falsify the active hypothesis.
EOF
