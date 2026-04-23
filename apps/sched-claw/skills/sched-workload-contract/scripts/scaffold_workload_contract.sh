#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scaffold_workload_contract.sh --output PATH --workload NAME
                                [--selector-kind script|pid|uid|gid|cgroup]
                                [--selector-value VALUE]
                                [--launcher-arg ARG]
                                [--launcher-env KEY=VALUE]
                                [--primary-metric NAME]
                                [--primary-goal minimize|maximize]
                                [--basis direct|proxy_estimate]
                                [--guardrail NAME:GOAL[:LIMIT]]
                                [--proxy-metric NAME:GOAL]

Examples:
  scaffold_workload_contract.sh --output artifacts/contract.toml --workload llvm \
    --selector-kind script --selector-value scripts/workloads/run-llvm-clang-build.sh \
    --primary-metric wall_time_s --primary-goal minimize --basis direct
EOF
}

output_path=""
workload_name=""
selector_kind=""
selector_value=""
primary_metric=""
primary_goal="minimize"
basis="direct"
launcher_args=()
launcher_env=()
guardrails=()
proxy_metrics=()

while (($# > 0)); do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --output)
      output_path="${2:?missing value for --output}"
      shift 2
      ;;
    --workload)
      workload_name="${2:?missing value for --workload}"
      shift 2
      ;;
    --selector-kind)
      selector_kind="${2:?missing value for --selector-kind}"
      shift 2
      ;;
    --selector-value)
      selector_value="${2:?missing value for --selector-value}"
      shift 2
      ;;
    --launcher-arg)
      launcher_args+=("${2:?missing value for --launcher-arg}")
      shift 2
      ;;
    --launcher-env)
      launcher_env+=("${2:?missing value for --launcher-env}")
      shift 2
      ;;
    --primary-metric)
      primary_metric="${2:?missing value for --primary-metric}"
      shift 2
      ;;
    --primary-goal)
      primary_goal="${2:?missing value for --primary-goal}"
      shift 2
      ;;
    --basis)
      basis="${2:?missing value for --basis}"
      shift 2
      ;;
    --guardrail)
      guardrails+=("${2:?missing value for --guardrail}")
      shift 2
      ;;
    --proxy-metric)
      proxy_metrics+=("${2:?missing value for --proxy-metric}")
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$output_path" || -z "$workload_name" ]]; then
  echo "--output and --workload are required" >&2
  exit 2
fi
if [[ -n "$selector_kind" && -z "$selector_value" ]]; then
  echo "--selector-value is required when --selector-kind is set" >&2
  exit 2
fi
if [[ -n "$selector_value" && -z "$selector_kind" ]]; then
  echo "--selector-kind is required when --selector-value is set" >&2
  exit 2
fi
if [[ "$primary_goal" != "minimize" && "$primary_goal" != "maximize" ]]; then
  echo "unsupported --primary-goal: $primary_goal" >&2
  exit 2
fi
if [[ "$basis" != "direct" && "$basis" != "proxy_estimate" ]]; then
  echo "unsupported --basis: $basis" >&2
  exit 2
fi

mkdir -p "$(dirname "$output_path")"

quote_toml() {
  printf '"%s"' "${1//\"/\\\"}"
}

{
  printf '# workload contract scaffold\n'
  printf 'name = %s\n' "$(quote_toml "$workload_name")"
  if [[ -n "$selector_kind" ]]; then
    printf 'selector_kind = %s\n' "$(quote_toml "$selector_kind")"
    printf 'selector_value = %s\n' "$(quote_toml "$selector_value")"
  else
    printf '# selector_kind = "script|pid|uid|gid|cgroup"\n'
    printf '# selector_value = ""\n'
  fi
  if [[ -n "$primary_metric" ]]; then
    printf 'primary_metric = %s\n' "$(quote_toml "$primary_metric")"
  else
    printf '# primary_metric = "throughput|latency|wall_time|ipc|cpi"\n'
  fi
  printf 'primary_goal = %s\n' "$(quote_toml "$primary_goal")"
  printf 'performance_basis = %s\n' "$(quote_toml "$basis")"
  printf '\n'
  printf 'launcher_argv = ['
  for index in "${!launcher_args[@]}"; do
    [[ "$index" == 0 ]] || printf ', '
    quote_toml "${launcher_args[$index]}"
  done
  printf ']\n'
  printf 'launcher_env = ['
  for index in "${!launcher_env[@]}"; do
    [[ "$index" == 0 ]] || printf ', '
    quote_toml "${launcher_env[$index]}"
  done
  printf ']\n'
  printf 'guardrails = ['
  for index in "${!guardrails[@]}"; do
    [[ "$index" == 0 ]] || printf ', '
    quote_toml "${guardrails[$index]}"
  done
  printf ']\n'
  printf 'proxy_metrics = ['
  for index in "${!proxy_metrics[@]}"; do
    [[ "$index" == 0 ]] || printf ', '
    quote_toml "${proxy_metrics[$index]}"
  done
  printf ']\n'
  printf '\n'
  printf '# notes = [\n'
  printf '#   "steady_state",\n'
  printf '#   "single_tenant_or_noisy_host",\n'
  printf '#   "rollback_trigger"\n'
  printf '# ]\n'
} >"$output_path"
