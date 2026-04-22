#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"
workspace_root="$repo_root"
socket_path=""
log_capacity="1000"
allow_roots=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --workspace-root)
      workspace_root="$2"
      shift 2
      ;;
    --socket)
      socket_path="$2"
      shift 2
      ;;
    --allow-root)
      allow_roots+=("$2")
      shift 2
      ;;
    --log-capacity)
      log_capacity="$2"
      shift 2
      ;;
    --help|-h)
      cat <<'EOF'
Usage: start-root-daemon.sh [options]

Options:
  --workspace-root PATH   Workspace root that the daemon should manage.
  --socket PATH           Unix socket path for the daemon.
  --allow-root PATH       Additional allowed executable root. Repeatable.
  --log-capacity N        Number of log lines the daemon should retain.
EOF
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$socket_path" ]]; then
  socket_path="$workspace_root/.nanoclaw/apps/sched-claw/sched-claw.sock"
fi

if [[ ${#allow_roots[@]} -eq 0 ]]; then
  allow_roots=("$workspace_root")
fi

client_uid="$(id -u)"
client_gid="$(id -g)"

cargo build --manifest-path "$repo_root/apps/Cargo.toml" -p sched-claw --bin sched-claw-daemon
daemon_bin="$repo_root/apps/target/debug/sched-claw-daemon"

cmd=(
  sudo
  "$daemon_bin"
  serve
  --workspace-root "$workspace_root"
  --socket "$socket_path"
  --log-capacity "$log_capacity"
  --client-uid "$client_uid"
  --client-gid "$client_gid"
)

for root in "${allow_roots[@]}"; do
  cmd+=(--allow-root "$root")
done

printf 'starting root daemon with socket %s for client uid=%s gid=%s\n' \
  "$socket_path" "$client_uid" "$client_gid"
exec "${cmd[@]}"
