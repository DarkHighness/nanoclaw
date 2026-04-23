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
CONTEXT_PATH=""
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

Demo wrapper that writes a durable workload context and then calls sched-claw
to autotune a sysbench + MySQL workload.

Options:
  --sched-claw-bin <path>   sched-claw binary. Default: apps/target/debug/sched-claw
  --experiment-id <id>      Demo run label. Default: mysql-sysbench-<unix>
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
CONTEXT_PATH="$DEMO_ARTIFACT_DIR/workload-context.md"
mkdir -p "$WORKLOAD_ARTIFACT_DIR"

cat >"$CONTEXT_PATH" <<EOF
# MySQL Sysbench Demo Workload Context

- run_label: $EXPERIMENT_ID
- skill: mysql-sysbench-tuning
- workload_launcher: $WORKLOAD_SCRIPT
- workload_cwd: $REPO_ROOT
- workload_args:
  - --mode
  - $MODE
  - --artifact-dir
  - $WORKLOAD_ARTIFACT_DIR
  - --mysql-host
  - $MYSQL_HOST
  - --mysql-port
  - $MYSQL_PORT
  - --mysql-user
  - $MYSQL_USER
  - --mysql-password
  - $MYSQL_PASSWORD
  - --mysql-db
  - $MYSQL_DB
  - --tables
  - $TABLES
  - --table-size
  - $TABLE_SIZE
  - --threads
  - $THREADS
  - --time
  - $TIME_SECONDS
  - --warmup
  - $WARMUP_SECONDS
- artifact_dir: $WORKLOAD_ARTIFACT_DIR
- primary_metric: transactions_per_sec:maximize
- guardrail: p95_latency_ms:minimize:10
- optional_metric: queries_per_sec:maximize
- notes:
  - Prefer direct sysbench throughput and latency.
  - Use proxy counters only when direct metrics are unavailable or invalid.
  - Keep daemon activation bounded and conservative for mixed throughput/latency workloads.
EOF

PROMPT=$(cat <<EOF
Load the mysql-sysbench-tuning skill before acting.
Read the workload context at $CONTEXT_PATH before acting.
This is a sched-claw demo for automatically tuning a sysbench + MySQL workload.
Use the launcher script and artifact directory from that context file.
Treat transactions_per_sec and p95_latency_ms as direct metrics. Prefer throughput first, but do not hide latency regressions.
Use the daemon only after you have explicit rollout criteria and a candidate that fits mixed throughput-and-latency workloads.
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
