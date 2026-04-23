#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scaffold_sched_ext_candidate.sh --output DIR --candidate-id ID
                                  [--experiment-id ID] [--from-template PATH]

Creates a candidate source file, build script, and notes file. If --from-template
is provided, the template is copied first and only lightweight token replacement
for {candidate_id} and {experiment_id} is applied.
EOF
}

output_dir=""
candidate_id=""
experiment_id="manual"
template_path=""

while (($# > 0)); do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --output)
      output_dir="${2:?missing value for --output}"
      shift 2
      ;;
    --candidate-id)
      candidate_id="${2:?missing value for --candidate-id}"
      shift 2
      ;;
    --experiment-id)
      experiment_id="${2:?missing value for --experiment-id}"
      shift 2
      ;;
    --from-template)
      template_path="${2:?missing value for --from-template}"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$output_dir" || -z "$candidate_id" ]]; then
  usage >&2
  exit 2
fi

mkdir -p "$output_dir"
source_path="$output_dir/${candidate_id}.bpf.c"
object_path="$output_dir/${candidate_id}.bpf.o"
build_path="$output_dir/build.sh"
notes_path="$output_dir/README.md"

if [[ -n "$template_path" ]]; then
  sed \
    -e "s/{candidate_id}/${candidate_id}/g" \
    -e "s/{experiment_id}/${experiment_id}/g" \
    "$template_path" >"$source_path"
else
  cat >"$source_path" <<EOF
#include <scx/common.bpf.h>

char _license[] SEC("license") = "GPL";

/*
 * Minimal sched-claw scaffold for ${candidate_id}.
 * Fill in the struct_ops callbacks that match the current workload evidence.
 */
EOF
fi

cat >"$build_path" <<EOF
#!/usr/bin/env bash
set -euo pipefail
cd "\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
clang -O2 -g -target bpf -c "$(basename "$source_path")" -o "$(basename "$object_path")"
EOF
chmod +x "$build_path"

cat >"$notes_path" <<EOF
# ${candidate_id}

- experiment: ${experiment_id}
- source: $(basename "$source_path")
- object: $(basename "$object_path")
- next step: edit the source, then run ./build.sh and inspect compiler or verifier output
EOF

printf 'source=%s\nobject=%s\nbuild=%s\n' "$source_path" "$object_path" "$build_path"
