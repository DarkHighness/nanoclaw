#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "$0")" && pwd)
# Shared helpers keep the demo scripts and workload launchers aligned on dry-run
# semantics and command rendering.
source "$SCRIPT_DIR/../lib/demo-common.sh"

REPO_ROOT=$(cd -- "$SCRIPT_DIR/../../../.." && pwd)
STAMP=$(sched_claw_timestamp)

LLVM_SRC=""
BUILD_DIR="$REPO_ROOT/.nanoclaw/apps/sched-claw/workloads/llvm-clang-build/build"
ARTIFACT_DIR="$REPO_ROOT/.nanoclaw/apps/sched-claw/workloads/llvm-clang-build/artifacts/$STAMP"
TARGET="clang"
BUILD_TYPE="Release"
JOBS=$(sched_claw_nproc)
DRY_RUN=0
CLEAN=0
CONFIGURE_ONLY=0
BUILD_ONLY=0
CMAKE_ARGS=()
BUILD_ARGS=()

usage() {
  cat <<'EOF'
Usage: run-llvm-clang-build.sh --llvm-src <path> [options]

One-click LLVM/clang build workload launcher for sched-claw demos.

Options:
  --llvm-src <path>       LLVM monorepo root or llvm/ source root.
  --build-dir <path>      CMake build directory.
  --artifact-dir <path>   Artifact directory for logs and metrics.
  --target <name>         Build target. Default: clang
  --build-type <type>     CMake build type. Default: Release
  --jobs <n>              Parallel build jobs. Default: detected CPU count
  --cmake-arg <arg>       Extra argument passed to cmake configure.
  --build-arg <arg>       Extra argument passed after cmake --build --.
  --clean                 Remove the build directory before configure.
  --configure-only        Run configure only.
  --build-only            Run build only.
  --dry-run               Print commands without executing them.
  --help                  Show this help text.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --llvm-src)
      LLVM_SRC="$2"
      shift 2
      ;;
    --build-dir)
      BUILD_DIR="$2"
      shift 2
      ;;
    --artifact-dir)
      ARTIFACT_DIR="$2"
      shift 2
      ;;
    --target)
      TARGET="$2"
      shift 2
      ;;
    --build-type)
      BUILD_TYPE="$2"
      shift 2
      ;;
    --jobs)
      JOBS="$2"
      shift 2
      ;;
    --cmake-arg)
      CMAKE_ARGS+=("$2")
      shift 2
      ;;
    --build-arg)
      BUILD_ARGS+=("$2")
      shift 2
      ;;
    --clean)
      CLEAN=1
      shift
      ;;
    --configure-only)
      CONFIGURE_ONLY=1
      shift
      ;;
    --build-only)
      BUILD_ONLY=1
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
[ "$CONFIGURE_ONLY" = 0 ] || [ "$BUILD_ONLY" = 0 ] || \
  sched_claw_fail "--configure-only and --build-only cannot be combined"

if [ -f "$LLVM_SRC/llvm/CMakeLists.txt" ]; then
  SOURCE_DIR="$LLVM_SRC/llvm"
elif [ -f "$LLVM_SRC/CMakeLists.txt" ]; then
  SOURCE_DIR="$LLVM_SRC"
else
  sched_claw_fail "could not find LLVM CMakeLists.txt under $LLVM_SRC"
fi

CONFIGURE_CMD=(
  cmake
  -G Ninja
  -S "$SOURCE_DIR"
  -B "$BUILD_DIR"
  -DLLVM_ENABLE_PROJECTS=clang
  -DCMAKE_BUILD_TYPE="$BUILD_TYPE"
)
for arg in "${CMAKE_ARGS[@]}"; do
  CONFIGURE_CMD+=("$arg")
done

BUILD_CMD=(cmake --build "$BUILD_DIR" --target "$TARGET")
if [ ${#BUILD_ARGS[@]} -gt 0 ]; then
  BUILD_CMD+=(-- "${BUILD_ARGS[@]}")
else
  BUILD_CMD+=(-- -j "$JOBS")
fi

printf 'artifact_dir=%s\n' "$ARTIFACT_DIR"
printf 'build_dir=%s\n' "$BUILD_DIR"
printf 'target=%s\n' "$TARGET"
printf 'jobs=%s\n' "$JOBS"

if [ "$DRY_RUN" = 1 ]; then
  [ "$CLEAN" = 0 ] || sched_claw_print_cmd rm -rf "$BUILD_DIR"
  [ "$BUILD_ONLY" = 1 ] || sched_claw_print_cmd "${CONFIGURE_CMD[@]}"
  [ "$CONFIGURE_ONLY" = 1 ] || sched_claw_print_cmd "${BUILD_CMD[@]}"
  exit 0
fi

sched_claw_require_command cmake
sched_claw_require_command ninja

[ "$CLEAN" = 0 ] || rm -rf "$BUILD_DIR"
mkdir -p "$ARTIFACT_DIR"

{
  printf '# configure\n'
  sched_claw_command_string "${CONFIGURE_CMD[@]}"
  printf '\n# build\n'
  sched_claw_command_string "${BUILD_CMD[@]}"
  printf '\n'
} >"$ARTIFACT_DIR/commands.sh"

configure_seconds=0
build_seconds=0

if [ "$BUILD_ONLY" = 0 ]; then
  start_ts=$(date +%s)
  "${CONFIGURE_CMD[@]}" 2>&1 | tee "$ARTIFACT_DIR/configure.log"
  end_ts=$(date +%s)
  configure_seconds=$((end_ts - start_ts))
fi

if [ "$CONFIGURE_ONLY" = 0 ]; then
  start_ts=$(date +%s)
  "${BUILD_CMD[@]}" 2>&1 | tee "$ARTIFACT_DIR/build.log"
  end_ts=$(date +%s)
  build_seconds=$((end_ts - start_ts))
fi

cat >"$ARTIFACT_DIR/metrics.env" <<EOF
configure_seconds=$configure_seconds
build_seconds=$build_seconds
primary_metric=build_seconds
proxy_metric_hints=ipc,cpi
EOF

printf 'metrics_file=%s\n' "$ARTIFACT_DIR/metrics.env"
