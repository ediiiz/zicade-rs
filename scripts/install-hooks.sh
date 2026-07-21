#!/usr/bin/env bash
# Install the repo git hooks into this clone's hooks directory.
# Safe to run from any worktree (git resolves the shared hooks path).
set -euo pipefail

hooks_dir="$(git rev-parse --git-path hooks)"
repo="$(git rev-parse --show-toplevel)"

cp "$repo/scripts/pre-commit" "$hooks_dir/pre-commit"
chmod +x "$hooks_dir/pre-commit"
echo "installed pre-commit -> $hooks_dir/pre-commit"
