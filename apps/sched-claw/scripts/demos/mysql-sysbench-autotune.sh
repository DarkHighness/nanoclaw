#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "$0")" && pwd)
source "$SCRIPT_DIR/../lib/demo-common.sh"

REPO_ROOT=$(cd -- "$SCRIPT_DIR/../../../.." && pwd)
STAMP=$(date +%s)

SCHED_CLAW_BIN="${SCHED_CLAW_BIN:-$REPO_ROOT/apps/target/debug/sched-claw}"
WORKLOAD_SCRIPT="$REPO_ROOT/apps/sched-claw/scripts/workloads/run-mysql-sysbench.sh"
EXPERIMENT_ID="mysql-sysbench-$STAMP"
WORKLOAD_ARTIFACT_DIR="$REPO_ROOT/.nanoclaw/apps/sched-claw/workloads/mysql-sysbench/artifacts/$STAMP"
DEMO_ARTIFACT_DIR="$REPO_ROOT/.nanoclaw/apps/sched-claw/demo/mysql-sysbench/$STAMP"
MODE="docker"
MYSQL_HOST="127.0.0.1"
MYSQL_PORT="3306"
MYSQL_USER="root"
MYSQL_PASSWORD="root"
MYSQL_DB="sysbench"
TABLES="8"
TABLE_SIZE="100000"
THREADS="8"
TIME_SECONDS="60"
WARMUP_SECONDS="10"
NO_EXEC=0
DRY_RUN=0
EXTRA_PROMPT=""

usage() {
  cat <<'EOF'
Usage: mysql-sysbench-autotune.sh [options]

Demo wrapper that bootstraps a sched-claw experiment and then calls sched-claw
to autotune a sysbench + MySQL workload.

Options:
  --sched-claw-bin <path>   sched-claw binary. Default: apps/target/debug/sched-claw
  --experiment-id <id>      Experiment id. Default: mysql-sysbench-<unix>
  --artifact-dir <path>     Artifact directory passed to the workload launcher.
  --mode <docker|host>      Workload launcher mode. Default: docker
  --mysql-host <host>       MySQL host. Default: 127.0.0.1
  --mysql-port <port>       MySQL port. Default: 3306
  --mysql-user <user>       MySQL user. Default: root
  --mysql-password <pass>   MySQL password. Default: root
  --mysql-db <name>         MySQL database. Default: sysbench
  --tables <n>              sysbench tables. Default: 8
  --table-size <n>          Rows per table. Default: 100000
  --threads <n>             sysbench client threads. Default: 8
  --time <seconds>          Run duration. Default: 60
  --warmup <seconds>        Warmup duration. Default: 10
  --extra-prompt <text>     Extra prompt text appended to the autotune request.
  --no-exec                 Only bootstrap the experiment; do not invoke sched-claw exec.
  --dry-run                 Print the sched-claw commands without executing them.
  --help                    Show this help text.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --sched-claw-bin)
      SCHED_CLAW_BIN="$2"
      shift 2
      ;;
    --experiment-id)
      EXPERIMENT_ID="$2"
      shift 2
      ;;
    --artifact-dir)
      WORKLOAD_ARTIFACT_DIR="$2"
      shift 2
      ;;
    --mode)
      MODE="$2"
      shift 2
      ;;
    --mysql-host)
      MYSQL_HOST="$2"
      shift 2
      ;;
    --mysql-port)
      MYSQL_PORT="$2"
      shift 2
      ;;
    --mysql-user)
      MYSQL_USER="$2"
      shift 2
      ;;
    --mysql-password)
      MYSQL_PASSWORD="$2"
      shift 2
      ;;
    --mysql-db)
      MYSQL_DB="$2"
      shift 2
      ;;
    --tables)
      TABLES="$2"
      shift 2
      ;;
    --table-size)
      TABLE_SIZE="$2"
      shift 2
      ;;
    --threads)
      THREADS="$2"
      shift 2
      ;;
    --time)
      TIME_SECONDS="$2"
      shift 2
      ;;
    --warmup)
      WARMUP_SECONDS="$2"
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

if [ "$DRY_RUN" = 0 ] && [ ! -x "$SCHED_CLAW_BIN" ]; then
  sched_claw_require_command cargo
  cargo build --manifest-path "$REPO_ROOT/apps/Cargo.toml" -p sched-claw
fi

mkdir -p "$DEMO_ARTIFACT_DIR"

INIT_CMD=(
  "$SCHED_CLAW_BIN"
  experiment init
  --id "$EXPERIMENT_ID"
  --workload-name mysql-sysbench-demo
  --workload-description "sysbench + MySQL autotune demo driven by sched-claw"
  --workload-cwd "$REPO_ROOT"
  --workload-arg "$WORKLOAD_SCRIPT"
  --workload-arg --mode
  --workload-arg "$MODE"
  --workload-arg --artifact-dir
  --workload-arg "$WORKLOAD_ARTIFACT_DIR"
  --workload-arg --mysql-host
  --workload-arg "$MYSQL_HOST"
  --workload-arg --mysql-port
  --workload-arg "$MYSQL_PORT"
  --workload-arg --mysql-user
  --workload-arg "$MYSQL_USER"
  --workload-arg --mysql-password
  --workload-arg "$MYSQL_PASSWORD"
  --workload-arg --mysql-db
  --workload-arg "$MYSQL_DB"
  --workload-arg --tables
  --workload-arg "$TABLES"
  --workload-arg --table-size
  --workload-arg "$TABLE_SIZE"
  --workload-arg --threads
  --workload-arg "$THREADS"
  --workload-arg --time
  --workload-arg "$TIME_SECONDS"
  --workload-arg --warmup
  --workload-arg "$WARMUP_SECONDS"
  --primary-metric transactions_per_sec
  --primary-goal maximize
  --guardrail p95_latency_ms:minimize:10
  --performance-notes "Prefer direct sysbench throughput and latency. Use proxy counters only if the direct metrics are unavailable or invalid."
)

PROMPT=$(cat <<EOF
Load the mysql-sysbench-tuning skill before acting.
Use experiment $EXPERIMENT_ID. This is a sched-claw demo for automatically tuning a sysbench + MySQL workload.
The launcher script is $WORKLOAD_SCRIPT and it will write metrics under $WORKLOAD_ARTIFACT_DIR.
Treat transactions_per_sec and p95_latency_ms as direct metrics. Prefer throughput first, but do not hide latency regressions.
Use the daemon only after you have explicit rollout criteria and a candidate that fits mixed throughput-and-latency workloads.
$EXTRA_PROMPT
EOF
)
printf '%s\n' "$PROMPT" >"$DEMO_ARTIFACT_DIR/prompt.txt"

EXEC_CMD=("$SCHED_CLAW_BIN" exec "$PROMPT")

printf 'experiment_id=%s\n' "$EXPERIMENT_ID"
printf 'demo_artifact_dir=%s\n' "$DEMO_ARTIFACT_DIR"

if [ "$DRY_RUN" = 1 ]; then
  sched_claw_print_cmd "${INIT_CMD[@]}"
  if [ "$NO_EXEC" = 0 ]; then
    sched_claw_print_cmd "${EXEC_CMD[@]}"
  fi
  exit 0
fi

"${INIT_CMD[@]}"
if [ "$NO_EXEC" = 0 ]; then
  "${EXEC_CMD[@]}"
fi
