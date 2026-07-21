#!/usr/bin/env bash
# Guardrail: fail if any tracked Rust source file exceeds MAX_LINES.
#
# Clippy has no native per-file line lint, so this script is the structural
# guard against the prior build's 1,000-3,700 line files. Run in CI and from
# the pre-commit hook. When a file trips it, split it (that split is part of
# the TDD refactor step, not an afterthought).
set -euo pipefail

MAX_LINES="${ZICADE_MAX_FILE_LINES:-400}"
root="$(git rev-parse --show-toplevel)"
cd "$root"

fail=0
# Only our own source: crates/ and apps/. Skip target/ and generated code.
while IFS= read -r f; do
    [ -f "$f" ] || continue
    lines="$(wc -l < "$f" | tr -d '[:space:]')"
    if [ "$lines" -gt "$MAX_LINES" ]; then
        echo "FILE TOO LARGE: $f has $lines lines (max $MAX_LINES)" >&2
        fail=1
    fi
done < <(git ls-files 'crates/**/*.rs' 'apps/**/*.rs')

if [ "$fail" -ne 0 ]; then
    echo "" >&2
    echo "Split oversized files into focused modules/crates before committing." >&2
    exit 1
fi
echo "file-size guard OK (max ${MAX_LINES} lines/file)"
