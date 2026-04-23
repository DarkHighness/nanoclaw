#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  collect_sched_timeline.sh --output DIR [--driver host|daemon]
                            [--pid PID[,PID...]] [--uid UID] [--gid GID] [--cgroup PATH]
                            [--timeout SECONDS] [--perf-bin PATH]
                            [--sched-claw-bin PATH] [--daemon-socket PATH]
                            [--latency-by-pid] [--overwrite]
                            [--] command [args...]

Examples:
  collect_sched_timeline.sh --output artifacts/sched --timeout 15 -- -- make -j32
  collect_sched_timeline.sh --output artifacts/sched --pid 4242 --timeout 10
  collect_sched_timeline.sh --driver daemon --output artifacts/sched-root --cgroup work.slice --timeout 10
EOF
}

driver="host"
output_dir=""
selector_kind=""
selector_value=""
timeout_seconds=""
perf_bin="${PERF_BIN:-perf}"
sched_claw_bin="${SCHED_CLAW_BIN:-sched-claw}"
daemon_socket="${SCHED_CLAW_DAEMON_SOCKET:-}"
latency_by_pid="false"
overwrite="false"
command=()

while (($# > 0)); do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --driver)
      driver="${2:?missing value for --driver}"
      shift 2
      ;;
    --output)
      output_dir="${2:?missing value for --output}"
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
    --sched-claw-bin)
      sched_claw_bin="${2:?missing value for --sched-claw-bin}"
      shift 2
      ;;
    --daemon-socket)
      daemon_socket="${2:?missing value for --daemon-socket}"
      shift 2
      ;;
    --latency-by-pid)
      latency_by_pid="true"
      shift
      ;;
    --overwrite)
      overwrite="true"
      shift
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

if [[ "$driver" != "host" && "$driver" != "daemon" ]]; then
  echo "unsupported driver: $driver" >&2
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

run_sched_reports() {
  local output_dir="$1"
  local latency_by_pid="$2"
  local latency_args=()
  local timehist_command="$output_dir/perf.sched.timehist.command.txt"
  local latency_command="$output_dir/perf.sched.latency.command.txt"
  if [[ "$latency_by_pid" == "true" ]]; then
    latency_args=(-p)
  fi
  {
    printf 'cwd=%s\n' "$PWD"
    printf '%q ' "$perf_bin" sched timehist -i "$output_dir/perf.sched.data"
    printf '\n'
  } >"$timehist_command"
  {
    printf 'cwd=%s\n' "$PWD"
    printf '%q ' "$perf_bin" sched latency -i "$output_dir/perf.sched.data" "${latency_args[@]}"
    printf '\n'
  } >"$latency_command"
  "$perf_bin" sched timehist -i "$output_dir/perf.sched.data" >"$output_dir/perf.sched.timehist.txt" 2>"$output_dir/perf.sched.timehist.stderr.log"
  "$perf_bin" sched latency -i "$output_dir/perf.sched.data" "${latency_args[@]}" >"$output_dir/perf.sched.latency.txt" 2>"$output_dir/perf.sched.latency.stderr.log"
}

if [[ "$driver" == "daemon" ]]; then
  if [[ -z "$selector_kind" ]]; then
    echo "daemon driver requires --pid, --uid, --gid, or --cgroup" >&2
    exit 2
  fi
  if ((${#command[@]} > 0)); then
    echo "daemon driver does not support command execution after --; use a selector-based capture" >&2
    exit 2
  fi

  output_parent="$(dirname "$output_dir")"
  output_base="$(basename "$output_dir")"
  mkdir -p "$output_parent"
  stderr_path="$output_parent/.${output_base}.collector.stderr.log"
  stdout_path="$output_parent/.${output_base}.collector.stdout.log"
  command_path="$output_parent/.${output_base}.collector.command.txt"
  selector_path="$output_parent/.${output_base}.selector.txt"

  daemon_args=()
  if [[ -n "$daemon_socket" ]]; then
    daemon_args+=(--daemon-socket "$daemon_socket")
  fi
  daemon_args+=(daemon collect-sched --output-dir "$output_dir")
  daemon_args+=(--duration-ms "$((timeout_seconds * 1000))")
  daemon_args+=(--style plain)
  daemon_args+=("--$selector_kind" "$selector_value")
  if [[ "$latency_by_pid" == "true" ]]; then
    daemon_args+=(--latency-by-pid)
  fi
  if [[ "$overwrite" == "true" ]]; then
    daemon_args+=(--overwrite)
  fi

  {
    printf 'cwd=%s\n' "$PWD"
    printf '%q ' "$sched_claw_bin" "${daemon_args[@]}"
    printf '\n'
  } >"$command_path"
  printf 'selector=%s\nvalue=%s\n' "$selector_kind" "$selector_value" >"$selector_path"
  "$sched_claw_bin" "${daemon_args[@]}" >"$stdout_path" 2>"$stderr_path"
  mv "$command_path" "$output_dir/collector.command.txt"
  mv "$selector_path" "$output_dir/selector.txt"
  mv "$stdout_path" "$output_dir/collector.stdout.log"
  mv "$stderr_path" "$output_dir/collector.stderr.log"
  exit 0
fi

mkdir -p "$output_dir"
record_stdout="$output_dir/perf.sched.record.stdout.log"
record_stderr="$output_dir/perf.sched.record.stderr.log"
record_command="$output_dir/perf.sched.record.command.txt"
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

record_args=(sched record -o "$output_dir/perf.sched.data" "${target_args[@]}")
if ((${#command[@]} > 0)); then
  record_args+=(-- "${command[@]}")
fi

{
  printf 'cwd=%s\n' "$PWD"
  printf '%q ' "$perf_bin" "${record_args[@]}"
  printf '\n'
} >"$record_command"

runner=("$perf_bin" "${record_args[@]}")
if [[ -n "$timeout_seconds" ]]; then
  runner=(timeout --signal=INT --kill-after=5s "${timeout_seconds}s" "${runner[@]}")
fi

"${runner[@]}" >"$record_stdout" 2>"$record_stderr"
run_sched_reports "$output_dir" "$latency_by_pid"
