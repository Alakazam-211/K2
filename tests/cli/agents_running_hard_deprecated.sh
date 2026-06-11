#!/usr/bin/env bash
# 0.39.0f Phase 2.1b — `k2so agents running` must hard-fail with a
# pointer to `k2so workspace list --running`.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2SO_CLI="$PROJECT_ROOT/cli/k2so"

OUTPUT_FILE="$(mktemp -t k2so-2.1b-agents-running-XXXXXX)"
trap "rm -f '$OUTPUT_FILE'" EXIT

if K2SO_PORT=1 "$K2SO_CLI" agents running 2>"$OUTPUT_FILE"; then
    echo "FAIL: k2so agents running exited 0; expected non-zero" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

if ! grep -qiE "workspace list --running" "$OUTPUT_FILE"; then
    echo "FAIL: stderr did not mention 'workspace list --running'" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

echo "OK: agents running hard-deprecated"
