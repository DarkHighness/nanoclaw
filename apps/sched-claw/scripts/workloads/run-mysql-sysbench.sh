#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "$0")" && pwd)
source "$SCRIPT_DIR/../lib/demo-common.sh"

REPO_ROOT=$(cd -- "$SCRIPT_DIR/../../../.." && pwd)
STAMP=$(sched_claw_timestamp)

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
REPORT_INTERVAL="5"
ARTIFACT_DIR="$REPO_ROOT/.nanoclaw/apps/sched-claw/workloads/mysql-sysbench/artifacts/$STAMP"
DOCKER_CONTAINER="sched-claw-mysql-sysbench"
DOCKER_IMAGE="mysql:8.4"
REPLACE_EXISTING=1
STOP_DOCKER_ON_EXIT=1
SKIP_PREPARE=0
SKIP_CLEANUP=0
DRY_RUN=0

usage() {
  cat <<'EOF'
Usage: run-mysql-sysbench.sh [options]

One-click sysbench + MySQL workload launcher for sched-claw demos.

Options:
  --mode <docker|host>         MySQL source. Default: docker
  --mysql-host <host>          MySQL host. Default: 127.0.0.1
  --mysql-port <port>          MySQL port. Default: 3306
  --mysql-user <user>          MySQL user. Default: root
  --mysql-password <pass>      MySQL password. Default: root
  --mysql-db <name>            MySQL database. Default: sysbench
  --tables <n>                 Number of sysbench tables. Default: 8
  --table-size <n>             Rows per sysbench table. Default: 100000
  --threads <n>                Sysbench client threads. Default: 8
  --time <seconds>             Run duration. Default: 60
  --warmup <seconds>           Optional warmup duration. Default: 10
  --report-interval <seconds>  Sysbench report interval. Default: 5
  --artifact-dir <path>        Artifact directory for logs and metrics.
  --docker-container-name <n>  Docker container name. Default: sched-claw-mysql-sysbench
  --docker-image <image>       Docker image. Default: mysql:8.4
  --skip-prepare               Skip sysbench prepare.
  --skip-cleanup               Skip sysbench cleanup.
  --keep-docker                Keep docker container after run.
  --dry-run                    Print commands without executing them.
  --help                       Show this help text.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
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
    --report-interval)
      REPORT_INTERVAL="$2"
      shift 2
      ;;
    --artifact-dir)
      ARTIFACT_DIR="$2"
      shift 2
      ;;
    --docker-container-name)
      DOCKER_CONTAINER="$2"
      shift 2
      ;;
    --docker-image)
      DOCKER_IMAGE="$2"
      shift 2
      ;;
    --skip-prepare)
      SKIP_PREPARE=1
      shift
      ;;
    --skip-cleanup)
      SKIP_CLEANUP=1
      shift
      ;;
    --keep-docker)
      STOP_DOCKER_ON_EXIT=0
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

[ "$MODE" = "docker" ] || [ "$MODE" = "host" ] || sched_claw_fail "--mode must be docker or host"

SYSBENCH_BASE=(
  sysbench
  oltp_read_write
  --mysql-host="$MYSQL_HOST"
  --mysql-port="$MYSQL_PORT"
  --mysql-user="$MYSQL_USER"
  --mysql-password="$MYSQL_PASSWORD"
  --mysql-db="$MYSQL_DB"
  --tables="$TABLES"
  --table-size="$TABLE_SIZE"
)
PREPARE_CMD=("${SYSBENCH_BASE[@]}" prepare)
RUN_CMD=(
  "${SYSBENCH_BASE[@]}"
  --threads="$THREADS"
  --time="$TIME_SECONDS"
  --report-interval="$REPORT_INTERVAL"
  run
)
WARMUP_CMD=(
  "${SYSBENCH_BASE[@]}"
  --threads="$THREADS"
  --time="$WARMUP_SECONDS"
  --report-interval="$REPORT_INTERVAL"
  run
)
CLEANUP_CMD=("${SYSBENCH_BASE[@]}" cleanup)
DOCKER_RUN_CMD=(
  docker run -d
  --name "$DOCKER_CONTAINER"
  -e "MYSQL_ROOT_PASSWORD=$MYSQL_PASSWORD"
  -e "MYSQL_DATABASE=$MYSQL_DB"
  -p "$MYSQL_PORT:3306"
  "$DOCKER_IMAGE"
  --skip-log-bin
  --innodb-flush-log-at-trx-commit=2
)

printf 'artifact_dir=%s\n' "$ARTIFACT_DIR"
printf 'mode=%s\n' "$MODE"
printf 'primary_metric=transactions_per_sec\n'
printf 'latency_guardrail=p95_latency_ms\n'

if [ "$DRY_RUN" = 1 ]; then
  if [ "$MODE" = "docker" ]; then
    [ "$REPLACE_EXISTING" = 0 ] || sched_claw_print_cmd docker rm -f "$DOCKER_CONTAINER"
    sched_claw_print_cmd "${DOCKER_RUN_CMD[@]}"
    sched_claw_print_cmd docker exec "$DOCKER_CONTAINER" mysqladmin ping -uroot "-p$MYSQL_PASSWORD" --silent
  fi
  [ "$SKIP_PREPARE" = 1 ] || sched_claw_print_cmd "${PREPARE_CMD[@]}"
  if [ "$WARMUP_SECONDS" != "0" ]; then
    sched_claw_print_cmd "${WARMUP_CMD[@]}"
  fi
  sched_claw_print_cmd "${RUN_CMD[@]}"
  [ "$SKIP_CLEANUP" = 1 ] || sched_claw_print_cmd "${CLEANUP_CMD[@]}"
  if [ "$MODE" = "docker" ] && [ "$STOP_DOCKER_ON_EXIT" = 1 ]; then
    sched_claw_print_cmd docker rm -f "$DOCKER_CONTAINER"
  fi
  exit 0
fi

sched_claw_require_command sysbench
mkdir -p "$ARTIFACT_DIR"

cleanup_docker() {
  if [ "$MODE" = "docker" ] && [ "$STOP_DOCKER_ON_EXIT" = 1 ]; then
    docker rm -f "$DOCKER_CONTAINER" >/dev/null 2>&1 || true
  fi
}

trap cleanup_docker EXIT

if [ "$MODE" = "docker" ]; then
  sched_claw_require_command docker
  if [ "$REPLACE_EXISTING" = 1 ]; then
    docker rm -f "$DOCKER_CONTAINER" >/dev/null 2>&1 || true
  fi
  "${DOCKER_RUN_CMD[@]}" >"$ARTIFACT_DIR/docker-container-id.txt"
  ready=0
  for _ in $(seq 1 60); do
    if docker exec "$DOCKER_CONTAINER" mysqladmin ping -uroot "-p$MYSQL_PASSWORD" --silent >/dev/null 2>&1; then
      ready=1
      break
    fi
    sleep 1
  done
  [ "$ready" = 1 ] || sched_claw_fail "docker mysql did not become ready"
fi

{
  printf '# prepare\n'
  sched_claw_command_string "${PREPARE_CMD[@]}"
  printf '\n# warmup\n'
  sched_claw_command_string "${WARMUP_CMD[@]}"
  printf '\n# run\n'
  sched_claw_command_string "${RUN_CMD[@]}"
  printf '\n# cleanup\n'
  sched_claw_command_string "${CLEANUP_CMD[@]}"
  printf '\n'
} >"$ARTIFACT_DIR/commands.sh"

if [ "$SKIP_PREPARE" = 0 ]; then
  "${PREPARE_CMD[@]}" 2>&1 | tee "$ARTIFACT_DIR/prepare.log"
fi

if [ "$WARMUP_SECONDS" != "0" ]; then
  "${WARMUP_CMD[@]}" 2>&1 | tee "$ARTIFACT_DIR/warmup.log"
fi

"${RUN_CMD[@]}" 2>&1 | tee "$ARTIFACT_DIR/run.log"

transactions_per_sec=$(sed -n 's/.*transactions:[^()]*(\([0-9.]\+\) per sec.).*/\1/p' "$ARTIFACT_DIR/run.log" | tail -n 1)
queries_per_sec=$(sed -n 's/.*queries:[^()]*(\([0-9.]\+\) per sec.).*/\1/p' "$ARTIFACT_DIR/run.log" | tail -n 1)
avg_latency_ms=$(sed -n 's/.*avg:[[:space:]]*\([0-9.]\+\).*/\1/p' "$ARTIFACT_DIR/run.log" | tail -n 1)
p95_latency_ms=$(sed -n 's/.*95th percentile:[[:space:]]*\([0-9.]\+\).*/\1/p' "$ARTIFACT_DIR/run.log" | tail -n 1)

cat >"$ARTIFACT_DIR/metrics.env" <<EOF
transactions_per_sec=${transactions_per_sec:-}
queries_per_sec=${queries_per_sec:-}
avg_latency_ms=${avg_latency_ms:-}
p95_latency_ms=${p95_latency_ms:-}
primary_metric=transactions_per_sec
latency_guardrail=p95_latency_ms
EOF

if [ "$SKIP_CLEANUP" = 0 ]; then
  "${CLEANUP_CMD[@]}" 2>&1 | tee "$ARTIFACT_DIR/cleanup.log"
fi

printf 'metrics_file=%s\n' "$ARTIFACT_DIR/metrics.env"
