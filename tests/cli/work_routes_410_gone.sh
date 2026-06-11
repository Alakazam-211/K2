#!/usr/bin/env bash
# 0.39.0f Phase 2.1b — `/cli/work/*` routes must return HTTP 410 Gone
# with a body pointing at /cli/inbox/* and `help-deprecated`. Route
# entries are kept (not removed) so external callers get a clear
# signal instead of a silent 404 from the catch-all.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_sandbox_daemon.sh"

sandbox_daemon_start

probe() {
    local route="$1"
    local extra="${2:-}"
    local url="http://127.0.0.1:${K2SO_PORT}${route}?token=${K2SO_TOKEN}&project=${SANDBOX_HOME}"
    if [ -n "$extra" ]; then url="${url}&${extra}"; fi
    # -w writes the status code to stdout after the body; -o lets us
    # separate body from status.
    local body_file status
    body_file="$(mktemp -t k2so-2.1b-410-XXXXXX)"
    status="$(curl -s -o "$body_file" -w "%{http_code}" --connect-timeout 3 --max-time 10 "$url")"
    echo "  $route → HTTP $status"
    if [ "$status" != "410" ]; then
        echo "FAIL: $route returned $status; expected 410" >&2
        cat "$body_file" >&2
        rm -f "$body_file"
        exit 1
    fi
    if ! grep -qi "help-deprecated" "$body_file"; then
        echo "FAIL: $route body did not reference 'help-deprecated':" >&2
        cat "$body_file" >&2
        rm -f "$body_file"
        exit 1
    fi
    rm -f "$body_file"
}

# Routes that 2.1b retires.
probe "/cli/work/inbox"
probe "/cli/agents/work" "agent=foo"
probe "/cli/agents/work/create" "title=x&body=y"
probe "/cli/agents/work/move" "agent=foo&filename=bar&from=inbox&to=active"
# 0.39.0f Phase 2.1c — cross-workspace inbox delivery migrated to
# POST /cli/inbox/compose?project=<target>. Old route now 410.
probe "/cli/work/inbox/create" "workspace=${SANDBOX_HOME}&title=x&body=y"

echo "OK: all /cli/work/* and /cli/agents/work* routes return 410 Gone"
