#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  render_perf_report.sh --input PERF.DATA --output REPORT.TXT
                        [--script-output PERF.SCRIPT.TXT]
                        [--perf-bin PATH]
                        [--report-arg ARG]...
                        [--script-arg ARG]...

Examples:
  render_perf_report.sh --input artifacts/perf.data --output artifacts/perf.report.txt
  render_perf_report.sh --input artifacts/perf.data --output artifacts/perf.report.txt \
    --script-output artifacts/perf.script.txt --report-arg=--percent-limit=0.5
EOF
}

input_path=""
output_path=""
script_output=""
perf_bin="${PERF_BIN:-perf}"
report_args=()
script_args=()

while (($# > 0)); do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --input)
      input_path="${2:?missing value for --input}"
      shift 2
      ;;
    --output)
      output_path="${2:?missing value for --output}"
      shift 2
      ;;
    --script-output)
      script_output="${2:?missing value for --script-output}"
      shift 2
      ;;
    --perf-bin)
      perf_bin="${2:?missing value for --perf-bin}"
      shift 2
      ;;
    --report-arg)
      report_args+=("${2:?missing value for --report-arg}")
      shift 2
      ;;
    --script-arg)
      script_args+=("${2:?missing value for --script-arg}")
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$input_path" || -z "$output_path" ]]; then
  usage >&2
  exit 2
fi

mkdir -p "$(dirname "$output_path")"
"$perf_bin" report --stdio -i "$input_path" "${report_args[@]}" >"$output_path"

if [[ -n "$script_output" ]]; then
  mkdir -p "$(dirname "$script_output")"
  "$perf_bin" script -i "$input_path" "${script_args[@]}" >"$script_output"
fi
