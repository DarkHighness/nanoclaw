#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"
workspace_root="${1:-$repo_root}"
state_dir="$workspace_root/.nanoclaw/apps/sched-claw"
mkdir -p "$state_dir"

smoke_dir="$(mktemp -d "$state_dir/smoke.XXXXXX")"
socket_path="$smoke_dir/smoke.sock"
daemon_log="$smoke_dir/daemon.log"
mock_loader="$smoke_dir/mock_scx_loader.sh"

cleanup() {
  if [[ -n "${daemon_pid:-}" ]]; then
    kill "$daemon_pid" >/dev/null 2>&1 || true
    wait "$daemon_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$smoke_dir"
}
trap cleanup EXIT

cat >"$mock_loader" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
trap 'echo trapped; exit 0' TERM INT
echo "mock-loader-start $*"
i=0
while [[ "$i" -lt 20 ]]; do
  echo "mock-loader-tick:$i"
  i=$((i + 1))
  sleep 0.1
done
echo "mock-loader-complete"
EOF
chmod +x "$mock_loader"

cargo build --manifest-path "$repo_root/apps/Cargo.toml" -p sched-claw --bin sched-claw --bin sched-claw-daemon
daemon_bin="$repo_root/apps/target/debug/sched-claw-daemon"
client_bin="$repo_root/apps/target/debug/sched-claw"

"$daemon_bin" serve \
  --workspace-root "$workspace_root" \
  --socket "$socket_path" \
  --allow-root "$workspace_root" \
  --allow-root "$smoke_dir" \
  >"$daemon_log" 2>&1 &
daemon_pid=$!

for _ in $(seq 1 100); do
  if "$client_bin" --daemon-socket "$socket_path" daemon status >/dev/null 2>&1; then
    break
  fi
  sleep 0.05
done

echo "== daemon status (initial) =="
"$client_bin" --daemon-socket "$socket_path" daemon status

echo "== activate mock loader =="
"$client_bin" --daemon-socket "$socket_path" daemon activate \
  --label smoke-loader \
  --cwd "$workspace_root" \
  -- "$mock_loader" --demo

sleep 0.3

echo "== daemon status (active) =="
"$client_bin" --daemon-socket "$socket_path" daemon status

echo "== daemon logs (active) =="
"$client_bin" --daemon-socket "$socket_path" daemon logs --tail-lines 20

echo "== stop active deployment =="
"$client_bin" --daemon-socket "$socket_path" daemon stop --graceful-timeout-ms 2000

echo "== daemon status (final) =="
"$client_bin" --daemon-socket "$socket_path" daemon status
