#!/bin/sh
set -eu

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

chmod +x .githooks/pre-commit .githooks/commit-msg
git config core.hooksPath .githooks

echo "Installed repository git hooks from .githooks"

