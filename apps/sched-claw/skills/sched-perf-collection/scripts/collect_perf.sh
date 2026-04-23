#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  collect_perf.sh --output DIR [--mode stat|record] [--events EVENT[,EVENT...]]
                  [--pid PID[,PID...]] [--uid UID] [--gid GID] [--cgroup PATH]
                  [--timeout SECONDS] [--perf-bin PATH] [--] command [args...]

Examples:
  collect_perf.sh --output artifacts/perf --timeout 15 -- -- make -j32
  collect_perf.sh --output artifacts/perf --pid 4242 --timeout 10
  collect_perf.sh --output artifacts/perf --cgroup work.slice --mode record --timeout 20
EOF
}

mode="stat"
output_dir=""
events=""
selector_kind=""
selector_value=""
timeout_seconds=""
perf_bin="${PERF_BIN:-perf}"
command=()

while (($# > 0)); do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --mode)
      mode="${2:?missing value for --mode}"
      shift 2
      ;;
    --output)
      output_dir="${2:?missing value for --output}"
      shift 2
      ;;
    --events)
      events="${2:?missing value for --events}"
      shift 2
      ;;
    --pid|--uid|--gid|--cgroup)
      if [[ -n "$selector_kind" ]]; then
        echo "only one selector is allowed" >&2
        exit 2
      fi
      selector_kind="${1#--}"
      selector_value="${2:?missing selector value}"
      shift 2
      ;;
    --timeout)
      timeout_seconds="${2:?missing value for --timeout}"
      shift 2
      ;;
    --perf-bin)
      perf_bin="${2:?missing value for --perf-bin}"
      shift 2
      ;;
    --)
      shift
      command=("$@")
      break
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$output_dir" ]]; then
  echo "--output is required" >&2
  exit 2
fi

if [[ -n "$selector_kind" && ${#command[@]} -gt 0 ]]; then
  echo "do not mix selector-based capture and command execution in one call" >&2
  exit 2
fi

if [[ -z "$selector_kind" && ${#command[@]} -eq 0 ]]; then
  echo "either provide a selector or a command after --" >&2
  exit 2
fi

if [[ -n "$selector_kind" && -z "$timeout_seconds" ]]; then
  echo "--timeout is required when capturing an existing pid, uid, gid, or cgroup" >&2
  exit 2
fi

if [[ "$mode" != "stat" && "$mode" != "record" ]]; then
  echo "unsupported mode: $mode" >&2
  exit 2
fi

resolve_pids() {
  local kind="$1"
  local value="$2"
  case "$kind" in
    pid)
      printf '%s\n' "$value"
      ;;
    uid|gid)
      local field
      if [[ "$kind" == "uid" ]]; then
        field="Uid"
      else
        field="Gid"
      fi
      for status in /proc/[0-9]*/status; do
        [[ -r "$status" ]] || continue
        local pid
        pid="$(basename "$(dirname "$status")")"
        local resolved=""
        resolved="$(awk -F':' -v field="$field" '$1 == field {gsub(/^[ \t]+|[ \t]+$/, "", $2); split($2, values, /[ \t]+/); print values[1]; exit}' "$status" 2>/dev/null || true)"
        if [[ "$resolved" == "$value" ]]; then
          printf '%s\n' "$pid"
        fi
      done
      ;;
    cgroup)
      local cgroup_path="$value"
      if [[ ! "$cgroup_path" = /* ]]; then
        cgroup_path="/sys/fs/cgroup/$cgroup_path"
      fi
      local procs_file="$cgroup_path"
      if [[ -d "$cgroup_path" ]]; then
        procs_file="$cgroup_path/cgroup.procs"
      fi
      [[ -r "$procs_file" ]] || return 0
      cat "$procs_file"
      ;;
    *)
      return 1
      ;;
  esac
}

mkdir -p "$output_dir"

stderr_path="$output_dir/collector.stderr.log"
stdout_path="$output_dir/collector.stdout.log"
command_path="$output_dir/collector.command.txt"
selector_path="$output_dir/selector.txt"

target_args=()
if [[ -n "$selector_kind" ]]; then
  mapfile -t selector_pids < <(resolve_pids "$selector_kind" "$selector_value" | awk 'NF')
  if ((${#selector_pids[@]} == 0)); then
    echo "no pids resolved for ${selector_kind}=${selector_value}" >&2
    exit 2
  fi
  printf 'selector=%s\nvalue=%s\npids=%s\n' \
    "$selector_kind" \
    "$selector_value" \
    "$(IFS=,; echo "${selector_pids[*]}")" >"$selector_path"
  target_args=(-p "$(IFS=,; echo "${selector_pids[*]}")")
fi

perf_args=()
if [[ "$mode" == "stat" ]]; then
  perf_args+=(stat -x, --no-big-num -o "$output_dir/perf.stat.csv")
  if [[ -n "$events" ]]; then
    perf_args+=(-e "$events")
  fi
else
  perf_args+=(record -o "$output_dir/perf.data")
  if [[ -n "$events" ]]; then
    perf_args+=(-e "$events")
  fi
fi
perf_args+=("${target_args[@]}")
if ((${#command[@]} > 0)); then
  perf_args+=(-- "${command[@]}")
fi

{
  printf 'cwd=%s\n' "$PWD"
  printf '%q ' "$perf_bin" "${perf_args[@]}"
  printf '\n'
} >"$command_path"

runner=("$perf_bin" "${perf_args[@]}")
if [[ -n "$timeout_seconds" ]]; then
  runner=(timeout --signal=INT --kill-after=5s "${timeout_seconds}s" "${runner[@]}")
fi

"${runner[@]}" >"$stdout_path" 2>"$stderr_path"
