#!/usr/bin/env bash
# `k2 help-deprecated` must list the hand-written deprecation stubs that
# still run (state, status, app-update, the heartbeat pointers, --agent)
# and the two verbs a daemon 410 points at. The retired-verb table rows
# are checked one by one in retired_verbs_table.sh. A docs drift check:
# fail loudly.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2SO_CLI="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

OUTPUT_FILE="$(mktemp -t k2so-2.1b-helpdep-XXXXXX)"
trap "rm -f '$OUTPUT_FILE'; hermetic_cli_cleanup" EXIT

if ! K2SO_PORT=1 "$K2SO_CLI" help-deprecated >"$OUTPUT_FILE" 2>&1; then
    echo "FAIL: help-deprecated exited non-zero" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

REQUIRED_VERBS=(
    "state list"
    "app-update"
    "status \"msg\""
    "heartbeat set"
    "heartbeat get"
    "heartbeat force"
    "--agent <name> flag"
    # /cli/agents/work and /cli/work/inbox answer 410 "see help-deprecated".
    "agents work"
    "work inbox"
)

MISSING=()
for verb in "${REQUIRED_VERBS[@]}"; do
    if ! grep -qF -- "$verb" "$OUTPUT_FILE"; then
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

echo "OK: help-deprecated lists all ${#REQUIRED_VERBS[@]} hand-written pointers"
