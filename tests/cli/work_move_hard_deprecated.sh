#!/usr/bin/env bash
# 0.39.0f Phase 2.1b — `k2so work move ...` must hard-fail with a
# pointer to `k2so inbox move`. No transition window.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2SO_CLI="$PROJECT_ROOT/cli/k2so"

OUTPUT_FILE="$(mktemp -t k2so-2.1b-work-move-XXXXXX)"
trap "rm -f '$OUTPUT_FILE'" EXIT

if K2SO_PORT=1 "$K2SO_CLI" work move --agent foo --file bar --from inbox --to active 2>"$OUTPUT_FILE"; then
    echo "FAIL: k2so work move exited 0; expected non-zero" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

if ! grep -qiE "inbox move" "$OUTPUT_FILE"; then
    echo "FAIL: stderr did not mention 'inbox move'" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

echo "OK: work move hard-deprecated"
