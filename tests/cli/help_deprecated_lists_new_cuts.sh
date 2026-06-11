#!/usr/bin/env bash
# 0.39.0f Phase 2.1b — `k2so help-deprecated` must list every verb
# that 2.1b hard-cut. This is a docs regression check: if someone
# adds a new hard-deprecation without updating the help page, the
# help-deprecated map drifts out of sync. Fail loudly.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2SO_CLI="$PROJECT_ROOT/cli/k2so"

OUTPUT_FILE="$(mktemp -t k2so-2.1b-helpdep-XXXXXX)"
trap "rm -f '$OUTPUT_FILE'" EXIT

if ! K2SO_PORT=1 "$K2SO_CLI" help-deprecated >"$OUTPUT_FILE" 2>&1; then
    echo "FAIL: help-deprecated exited non-zero" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

# Each verb cut in 2.1b must appear in the help page somewhere.
REQUIRED_VERBS=(
    "work create"
    "work inbox"
    "work move"
    "work send"
    "work done"
    "agents create"
    "agents delete"
    "agents list"
    "agents work"
    "agents lock"
    "agents unlock"
    "agents launch"
    "agents profile"
    "agents status"
    "agents generate-md"
    "agents running"
    "whatsnew"
)

MISSING=()
for verb in "${REQUIRED_VERBS[@]}"; do
    if ! grep -qF "$verb" "$OUTPUT_FILE"; then
        MISSING+=("$verb")
    fi
done

if [ ${#MISSING[@]} -gt 0 ]; then
    echo "FAIL: help-deprecated is missing these verbs:" >&2
    for v in "${MISSING[@]}"; do echo "  - $v" >&2; done
    echo "---" >&2
    echo "help-deprecated output was:" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

echo "OK: help-deprecated lists all ${#REQUIRED_VERBS[@]} cut verbs"
