#!/usr/bin/env bash
set -euo pipefail

sched_claw_note() {
  printf '%s\n' "$*" >&2
}

sched_claw_fail() {
  sched_claw_note "Error: $*"
  exit 1
}

sched_claw_timestamp() {
  date +%Y%m%d-%H%M%S
}

sched_claw_nproc() {
  if command -v nproc >/dev/null 2>&1; then
    nproc
  elif command -v getconf >/dev/null 2>&1; then
    getconf _NPROCESSORS_ONLN
  else
    printf '1\n'
  fi
}

sched_claw_require_command() {
  local cmd="$1"
  command -v "$cmd" >/dev/null 2>&1 || sched_claw_fail "missing required command: $cmd"
}

sched_claw_print_cmd() {
  printf '+ '
  printf '%q ' "$@"
  printf '\n'
}

sched_claw_run() {
  local dry_run="$1"
  shift
  if [ "$dry_run" = 1 ]; then
    sched_claw_print_cmd "$@"
  else
    "$@"
  fi
}

sched_claw_command_string() {
  printf '%q ' "$@"
}
