#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  capture_build_verifier_artifacts.sh --artifact-dir DIR --source PATH --object PATH
                                      [--build-script PATH | --build-command TEXT]
                                      [--verify-command TEXT]
                                      [--overwrite]

Examples:
  capture_build_verifier_artifacts.sh --artifact-dir artifacts/build-a \
    --source cand-a.bpf.c --object cand-a.bpf.o --build-script ./build.sh \
    --verify-command "bpftool -d -L prog loadall cand-a.bpf.o /sys/fs/bpf"
EOF
}

artifact_dir=""
source_path=""
object_path=""
build_script=""
build_command=""
verify_command=""
overwrite="false"

while (($# > 0)); do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --artifact-dir)
      artifact_dir="${2:?missing value for --artifact-dir}"
      shift 2
      ;;
    --source)
      source_path="${2:?missing value for --source}"
      shift 2
      ;;
    --object)
      object_path="${2:?missing value for --object}"
      shift 2
      ;;
    --build-script)
      build_script="${2:?missing value for --build-script}"
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
    --overwrite)
      overwrite="true"
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$artifact_dir" || -z "$source_path" || -z "$object_path" ]]; then
  echo "--artifact-dir, --source, and --object are required" >&2
  exit 2
fi
if [[ -n "$build_script" && -n "$build_command" ]]; then
  echo "choose exactly one of --build-script or --build-command" >&2
  exit 2
fi
if [[ -z "$build_script" && -z "$build_command" ]]; then
  echo "one of --build-script or --build-command is required" >&2
  exit 2
fi

if [[ -e "$artifact_dir" && ! -d "$artifact_dir" ]]; then
  echo "artifact dir is not a directory: $artifact_dir" >&2
  exit 2
fi
mkdir -p "$artifact_dir"
if [[ "$overwrite" != "true" ]] && find "$artifact_dir" -mindepth 1 -print -quit | grep -q .; then
  echo "artifact dir is not empty: $artifact_dir" >&2
  exit 2
fi

printf 'source=%s\nobject=%s\n' "$source_path" "$object_path" >"$artifact_dir/context.txt"

run_and_capture() {
  local command_text="$1"
  local stdout_path="$2"
  local stderr_path="$3"
  local status_path="$4"
  local cwd_path="$5"

  (
    set +e
    bash -lc "$command_text" >"$stdout_path" 2>"$stderr_path"
    local status=$?
    printf '%s\n' "$status" >"$status_path"
    exit 0
  )
  {
    printf 'cwd=%s\n' "$cwd_path"
    printf 'command=%s\n' "$command_text"
  }
}

if [[ -n "$build_script" ]]; then
  build_command="$build_script"
fi

run_and_capture \
  "$build_command" \
  "$artifact_dir/build.stdout.log" \
  "$artifact_dir/build.stderr.log" \
  "$artifact_dir/build.status" \
  "$PWD" >"$artifact_dir/build.command.txt"

build_status="$(cat "$artifact_dir/build.status")"

if [[ -n "$verify_command" ]]; then
  run_and_capture \
    "$verify_command" \
    "$artifact_dir/verify.stdout.log" \
    "$artifact_dir/verify.stderr.log" \
    "$artifact_dir/verify.status" \
    "$PWD" >"$artifact_dir/verify.command.txt"
  verify_status="$(cat "$artifact_dir/verify.status")"
else
  verify_status=""
fi

{
  printf 'build_status=%s\n' "$build_status"
  if [[ -n "$verify_status" ]]; then
    printf 'verify_status=%s\n' "$verify_status"
  fi
} >"$artifact_dir/summary.env"
