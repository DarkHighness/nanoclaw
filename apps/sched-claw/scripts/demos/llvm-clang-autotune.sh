#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "$0")" && pwd)
source "$SCRIPT_DIR/../lib/demo-common.sh"

REPO_ROOT=$(cd -- "$SCRIPT_DIR/../../../.." && pwd)
STAMP=$(date +%s)

LLVM_SRC=""
SCHED_CLAW_BIN="${SCHED_CLAW_BIN:-$REPO_ROOT/apps/target/debug/sched-claw}"
WORKLOAD_SCRIPT="$REPO_ROOT/apps/sched-claw/scripts/workloads/run-llvm-clang-build.sh"
EXPERIMENT_ID="llvm-clang-$STAMP"
BUILD_DIR="$REPO_ROOT/.nanoclaw/apps/sched-claw/workloads/llvm-clang-build/build-$STAMP"
WORKLOAD_ARTIFACT_DIR="$REPO_ROOT/.nanoclaw/apps/sched-claw/workloads/llvm-clang-build/artifacts/$STAMP"
DEMO_ARTIFACT_DIR="$REPO_ROOT/.nanoclaw/apps/sched-claw/demo/llvm-clang-build/$STAMP"
CONTEXT_PATH=""
JOBS=$(sched_claw_nproc)
TARGET="clang"
NO_EXEC=0
DRY_RUN=0
EXTRA_PROMPT=""

usage() {
  cat <<'EOF'
Usage: llvm-clang-autotune.sh --llvm-src <path> [options]

Demo wrapper that writes a durable workload context and then calls sched-claw
to autotune an LLVM/clang build workload.

Options:
  --llvm-src <path>         LLVM monorepo root or llvm/ source root.
  --sched-claw-bin <path>   sched-claw binary. Default: apps/target/debug/sched-claw
  --experiment-id <id>      Demo run label. Default: llvm-clang-<unix>
  --build-dir <path>        CMake build directory passed to the workload launcher.
  --artifact-dir <path>     Artifact directory passed to the workload launcher.
  --jobs <n>                Parallel build jobs. Default: detected CPU count
  --target <name>           Build target. Default: clang
  --extra-prompt <text>     Extra prompt text appended to the autotune request.
  --no-exec                 Only bootstrap the experiment; do not invoke sched-claw exec.
  --dry-run                 Print the sched-claw commands without executing them.
  --help                    Show this help text.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --llvm-src)
      LLVM_SRC="$2"
      shift 2
      ;;
    --sched-claw-bin)
      SCHED_CLAW_BIN="$2"
      shift 2
      ;;
    --experiment-id)
      EXPERIMENT_ID="$2"
      shift 2
      ;;
    --build-dir)
      BUILD_DIR="$2"
      shift 2
      ;;
    --artifact-dir)
      WORKLOAD_ARTIFACT_DIR="$2"
      shift 2
      ;;
    --jobs)
      JOBS="$2"
      shift 2
      ;;
    --target)
      TARGET="$2"
      shift 2
      ;;
    --extra-prompt)
      EXTRA_PROMPT="$2"
      shift 2
      ;;
    --no-exec)
      NO_EXEC=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      sched_claw_fail "unknown argument: $1"
      ;;
  esac
done

[ -n "$LLVM_SRC" ] || sched_claw_fail "--llvm-src is required"

if [ "$DRY_RUN" = 0 ] && [ ! -x "$SCHED_CLAW_BIN" ]; then
  sched_claw_require_command cargo
  cargo build --manifest-path "$REPO_ROOT/apps/Cargo.toml" -p sched-claw
fi

mkdir -p "$DEMO_ARTIFACT_DIR"
CONTEXT_PATH="$DEMO_ARTIFACT_DIR/workload-context.md"
mkdir -p "$WORKLOAD_ARTIFACT_DIR"

cat >"$CONTEXT_PATH" <<EOF
# LLVM/Clang Demo Workload Context

- run_label: $EXPERIMENT_ID
- skill: llvm-clang-build-tuning
- workload_launcher: $WORKLOAD_SCRIPT
- workload_cwd: $REPO_ROOT
- workload_args:
  - --llvm-src
  - $LLVM_SRC
  - --build-dir
  - $BUILD_DIR
  - --artifact-dir
  - $WORKLOAD_ARTIFACT_DIR
  - --jobs
  - $JOBS
  - --target
  - $TARGET
- artifact_dir: $WORKLOAD_ARTIFACT_DIR
- primary_metric: build_seconds:minimize
- secondary_metric: configure_seconds:minimize
- proxy_metrics:
  - ipc:maximize
  - cpi:minimize
- notes:
  - Prefer direct build wall-clock time.
  - Switch to proxy_estimate only when timing is contaminated or incomplete.
  - Keep daemon activation bounded and conservative for long-running builds.
EOF

PROMPT=$(cat <<EOF
Load the llvm-clang-build-tuning skill before acting.
Read the workload context at $CONTEXT_PATH before acting.
This is a sched-claw demo for automatically tuning an LLVM/clang build workload.
Use the launcher script and artifact directory from that context file.
Treat build_seconds as the primary direct metric. If direct timing is noisy or unavailable, explicitly state that you are using proxy_estimate and use ipc/cpi instead.
Prefer conservative sched-ext candidates that fit compile-heavy CPU-bound workloads, and keep rollout criteria explicit before touching the daemon.
$EXTRA_PROMPT
EOF
)
printf '%s\n' "$PROMPT" >"$DEMO_ARTIFACT_DIR/prompt.txt"

EXEC_CMD=("$SCHED_CLAW_BIN" exec "$PROMPT")

printf 'run_label=%s\n' "$EXPERIMENT_ID"
printf 'demo_artifact_dir=%s\n' "$DEMO_ARTIFACT_DIR"
printf 'context_path=%s\n' "$CONTEXT_PATH"

if [ "$DRY_RUN" = 1 ]; then
  if [ "$NO_EXEC" = 0 ]; then
    sched_claw_print_cmd "${EXEC_CMD[@]}"
  fi
  exit 0
fi

if [ "$NO_EXEC" = 0 ]; then
  "${EXEC_CMD[@]}"
fi
