#!/usr/bin/env bash
# 0.39.0f Phase 2.1b — `k2so work create` must hard-fail with a
# pointer to `k2so inbox compose`. No transition window — exit 1.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2SO_CLI="$PROJECT_ROOT/cli/k2so"

OUTPUT_FILE="$(mktemp -t k2so-2.1b-work-create-XXXXXX)"
trap "rm -f '$OUTPUT_FILE'" EXIT

# The CLI may try to contact the daemon for some verbs but `work
# create` hard-deps BEFORE any network call. Run with an
# intentionally unreachable port so we'd notice an accidental dial.
if K2SO_PORT=1 "$K2SO_CLI" work create --title "x" 2>"$OUTPUT_FILE"; then
    echo "FAIL: k2so work create exited 0; expected non-zero" >&2
    echo "stderr was:" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

if ! grep -qiE "inbox compose" "$OUTPUT_FILE"; then
    echo "FAIL: stderr did not mention 'inbox compose'" >&2
    echo "stderr was:" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

if ! grep -qi "help-deprecated" "$OUTPUT_FILE"; then
    echo "FAIL: stderr did not reference 'help-deprecated'" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

echo "OK: work create hard-deprecated with inbox compose pointer"
