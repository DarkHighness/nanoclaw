#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  collect_topology_snapshot.sh --output DIR [--driver host|daemon]
                               [--pid PID[,PID...]] [--uid UID] [--gid GID] [--cgroup PATH]
                               [--sched-claw-bin PATH] [--daemon-socket PATH] [--overwrite]

Examples:
  collect_topology_snapshot.sh --output artifacts/topology
  collect_topology_snapshot.sh --output artifacts/topology --pid 4242
  collect_topology_snapshot.sh --driver daemon --output artifacts/topology-root --cgroup work.slice
EOF
}

driver="host"
output_dir=""
selector_kind=""
selector_value=""
sched_claw_bin="${SCHED_CLAW_BIN:-sched-claw}"
daemon_socket="${SCHED_CLAW_DAEMON_SOCKET:-}"
overwrite="false"

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
    --sched-claw-bin)
      sched_claw_bin="${2:?missing value for --sched-claw-bin}"
      shift 2
      ;;
    --daemon-socket)
      daemon_socket="${2:?missing value for --daemon-socket}"
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

if [[ -z "$output_dir" ]]; then
  echo "--output is required" >&2
  exit 2
fi
if [[ "$driver" != "host" && "$driver" != "daemon" ]]; then
  echo "unsupported driver: $driver" >&2
  exit 2
fi

prepare_output_dir() {
  local dir="$1"
  local allow_overwrite="$2"
  if [[ -e "$dir" && ! -d "$dir" ]]; then
    echo "output path is not a directory: $dir" >&2
    exit 2
  fi
  mkdir -p "$dir"
  if [[ "$allow_overwrite" != "true" ]] && find "$dir" -mindepth 1 -print -quit | grep -q .; then
    echo "output directory is not empty: $dir" >&2
    exit 2
  fi
}

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

snapshot_file() {
  local src="$1"
  local dest="$2"
  [[ -r "$src" ]] || return 0
  mkdir -p "$(dirname "$dest")"
  cat "$src" >"$dest"
}

resolve_pid_cgroup() {
  local pid="$1"
  awk -F: '
    BEGIN { selected = "" }
    {
      if (NF < 3) {
        next
      }
      controllers = $2
      path = $3
      if (selected == "" && controllers == "") {
        selected = path
      } else if (selected == "" && controllers ~ /(^|,)cpuset(,|$)/) {
        selected = path
      }
    }
    END {
      if (selected != "") {
        print selected
      }
    }
  ' "/proc/$pid/cgroup" 2>/dev/null || true
}

capture_cgroup_context() {
  local raw_cgroup="$1"
  local root="$2"
  local cgroup_path="$raw_cgroup"
  if [[ ! "$cgroup_path" = /* ]]; then
    cgroup_path="/sys/fs/cgroup/${cgroup_path#/}"
  fi
  local label="${cgroup_path#/sys/fs/cgroup/}"
  label="${label#/}"
  if [[ -z "$label" ]]; then
    label="root"
  fi
  label="${label//\//__}"
  label="${label//./_}"
  local out_dir="$root/$label"
  mkdir -p "$out_dir"
  snapshot_file "$cgroup_path/cpu.stat" "$out_dir/cpu.stat"
  snapshot_file "$cgroup_path/cpuset.cpus.effective" "$out_dir/cpuset.cpus.effective"
  snapshot_file "$cgroup_path/cpuset.mems.effective" "$out_dir/cpuset.mems.effective"
}

prepare_output_dir "$output_dir" "$overwrite"

if [[ "$driver" == "daemon" ]]; then
  output_parent="$(dirname "$output_dir")"
  output_base="$(basename "$output_dir")"
  stderr_path="$output_parent/.${output_base}.collector.stderr.log"
  stdout_path="$output_parent/.${output_base}.collector.stdout.log"
  command_path="$output_parent/.${output_base}.collector.command.txt"
  selector_path="$output_parent/.${output_base}.selector.txt"

  daemon_args=()
  if [[ -n "$daemon_socket" ]]; then
    daemon_args+=(--daemon-socket "$daemon_socket")
  fi
  daemon_args+=(daemon collect-topology --output-dir "$output_dir" --style plain)
  if [[ -n "$selector_kind" ]]; then
    daemon_args+=("--$selector_kind" "$selector_value")
  fi
  if [[ "$overwrite" == "true" ]]; then
    daemon_args+=(--overwrite)
  fi

  {
    printf 'cwd=%s\n' "$PWD"
    printf '%q ' "$sched_claw_bin" "${daemon_args[@]}"
    printf '\n'
  } >"$command_path"
  if [[ -n "$selector_kind" ]]; then
    printf 'selector=%s\nvalue=%s\n' "$selector_kind" "$selector_value" >"$selector_path"
  else
    printf 'selector=<none>\n' >"$selector_path"
  fi
  "$sched_claw_bin" "${daemon_args[@]}" >"$stdout_path" 2>"$stderr_path"
  mv "$command_path" "$output_dir/collector.command.txt"
  mv "$selector_path" "$output_dir/selector.txt"
  mv "$stdout_path" "$output_dir/collector.stdout.log"
  mv "$stderr_path" "$output_dir/collector.stderr.log"
  exit 0
fi

{
  printf 'driver=host\n'
  printf 'cwd=%s\n' "$PWD"
  if [[ -n "$selector_kind" ]]; then
    printf 'selector=%s\n' "$selector_kind"
    printf 'value=%s\n' "$selector_value"
  else
    printf 'selector=<none>\n'
  fi
} >"$output_dir/collector.command.txt"

if [[ -n "$selector_kind" ]]; then
  mapfile -t selector_pids < <(resolve_pids "$selector_kind" "$selector_value" | awk 'NF' | sort -u)
  if ((${#selector_pids[@]} == 0)); then
    echo "no pids resolved for ${selector_kind}=${selector_value}" >&2
    exit 2
  fi
  printf 'selector=%s\nvalue=%s\npids=%s\n' \
    "$selector_kind" \
    "$selector_value" \
    "$(IFS=,; echo "${selector_pids[*]}")" >"$output_dir/selector.txt"
else
  selector_pids=()
  printf 'selector=<none>\n' >"$output_dir/selector.txt"
fi

snapshot_file "/sys/devices/system/cpu/online" "$output_dir/sys.cpu.online"
snapshot_file "/sys/devices/system/cpu/possible" "$output_dir/sys.cpu.possible"
snapshot_file "/sys/devices/system/cpu/present" "$output_dir/sys.cpu.present"
snapshot_file "/sys/devices/system/cpu/smt/active" "$output_dir/sys.cpu.smt.active"
snapshot_file "/sys/devices/system/node/online" "$output_dir/sys.node.online"

for cpu_dir in /sys/devices/system/cpu/cpu[0-9]*; do
  [[ -d "$cpu_dir" ]] || continue
  cpu_name="$(basename "$cpu_dir")"
  topology_dir="$cpu_dir/topology"
  snapshot_file "$topology_dir/core_id" "$output_dir/cpus/$cpu_name/core_id"
  snapshot_file "$topology_dir/physical_package_id" "$output_dir/cpus/$cpu_name/physical_package_id"
  snapshot_file "$topology_dir/die_id" "$output_dir/cpus/$cpu_name/die_id"
  snapshot_file "$topology_dir/thread_siblings_list" "$output_dir/cpus/$cpu_name/thread_siblings_list"
  snapshot_file "$topology_dir/core_cpus_list" "$output_dir/cpus/$cpu_name/core_cpus_list"
done

declare -A seen_cgroups=()
if [[ "$selector_kind" == "cgroup" ]]; then
  if [[ "$selector_value" = /* ]]; then
    seen_cgroups["$selector_value"]=1
  else
    seen_cgroups["/sys/fs/cgroup/${selector_value#/}"]=1
  fi
fi

for pid in "${selector_pids[@]}"; do
  pid_dir="$output_dir/pids/$pid"
  snapshot_file "/proc/$pid/status" "$pid_dir/status.txt"
  snapshot_file "/proc/$pid/cgroup" "$pid_dir/cgroup.txt"
  resolved_cgroup="$(resolve_pid_cgroup "$pid")"
  if [[ -n "$resolved_cgroup" ]]; then
    seen_cgroups["/sys/fs/cgroup/${resolved_cgroup#/}"]=1
  fi
done

for cgroup_path in "${!seen_cgroups[@]}"; do
  capture_cgroup_context "$cgroup_path" "$output_dir/cgroups"
done
