#!/usr/bin/env bash
# `k2 help-deprecated` must list every 2.1b verb that still answers with
# a pointer, and must not list the ones that were removed (they now get
# the normal "Unknown command" answer). A docs drift check: fail loudly.
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

# Each 2.1b verb that still answers with a pointer must appear in the
# help page somewhere.
REQUIRED_VERBS=(
    "work create"
    "work move"
    "work send"
    "agents create"
    "agents running"
    "whatsnew"
    # No CLI stub any more, but /cli/agents/work and /cli/work/inbox still
    # answer 410 "see help-deprecated", so the map keeps their entries.
    "agents work"
    "work inbox"
)

# Verbs whose pointer was removed: the map must not offer them.
REMOVED_VERBS=(
    "work done"
    "agents delete"
    "agents list"
    "agents lock"
    "agents unlock"
    "agents launch"
    "agents profile"
    "agents status"
    "agents generate-md"
    "agents triage"
    "agents reap"
    "agentic"
    "companion"
    "feed"
    "roster"
    "signal"
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

LISTED=()
for verb in "${REMOVED_VERBS[@]}"; do
    if grep -qE "(^  |k2 )${verb}( |\$)" "$OUTPUT_FILE"; then
        LISTED+=("$verb")
    fi
done

if [ ${#LISTED[@]} -gt 0 ]; then
    echo "FAIL: help-deprecated still lists these removed verbs:" >&2
    for v in "${LISTED[@]}"; do echo "  - $v" >&2; done
    echo "---" >&2
    cat "$OUTPUT_FILE" >&2
    exit 1
fi

echo "OK: help-deprecated lists all ${#REQUIRED_VERBS[@]} kept pointers and none of the ${#REMOVED_VERBS[@]} removed verbs"
