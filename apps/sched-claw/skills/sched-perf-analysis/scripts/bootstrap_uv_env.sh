#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  bootstrap_uv_env.sh [--refresh] [VENV_DIR]

Creates or refreshes a uv-managed virtual environment for sched-claw analysis
helpers and prints the Python interpreter path on stdout.
EOF
}

refresh=0
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi
if [[ "${1:-}" == "--refresh" ]]; then
  refresh=1
  shift
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
venv_dir="${1:-$script_dir/.venv}"
uv_bin="${UV_BIN:-uv}"
python_bin="${PYTHON_BIN:-python3}"

if [[ $refresh -eq 1 ]]; then
  rm -rf "$venv_dir"
fi

"$uv_bin" venv --python "$python_bin" "$venv_dir"
"$uv_bin" pip install --python "$venv_dir/bin/python" -r "$script_dir/requirements.txt"
printf '%s\n' "$venv_dir/bin/python"
