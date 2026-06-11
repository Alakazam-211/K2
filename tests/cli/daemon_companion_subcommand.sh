#!/usr/bin/env bash
# 0.39.0f Phase 2.1c — `k2so daemon companion <start|stop|status>` must
# dispatch to the daemon's existing /cli/companion/{start,stop,status}
# routes (Unit 1 + Unit 7c). Phase 2.1b removed the top-level
# `k2so companion` verb and pointed users at the daemon subcommand,
# but the daemon dispatch arm had no `companion` case until 2.1c.
#
# This test probes the daemon routes directly (no `k2so` CLI invocation
# — the CLI hard-codes the daemon URL via $HOME/.k2so/daemon.port which
# the sandbox can't safely override). We assert:
#   1. /cli/companion/status reports running=false initially
#   2. /cli/companion/start returns ok=true (or a meaningful error)
#   3. /cli/companion/stop returns ok=true (idempotent)
#
# Companion start may fail in sandbox (no ngrok token); we treat that
# as a pass on the *route wiring* check — the point is the daemon
# handler responds with a structured JSON body, not a 404.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/_sandbox_daemon.sh"

sandbox_daemon_start

base="http://127.0.0.1:${K2SO_PORT}"
qs="token=${K2SO_TOKEN}&project=${SANDBOX_HOME}"

# ── 1. status initially → must be valid JSON with `running` key ──────
echo "→ GET /cli/companion/status (initial)"
status_body="$(curl -sf --connect-timeout 3 --max-time 10 \
    "${base}/cli/companion/status?${qs}")"
echo "  body: $status_body"
echo "$status_body" | python3 -c '
import json, sys
data = json.loads(sys.stdin.read())
if "running" not in data:
    print("FAIL: status response missing `running` field", file=sys.stderr)
    sys.exit(1)
# Default state is not-running.
if data["running"] is not False:
    print(f"FAIL: expected running=false on cold boot, got {data}", file=sys.stderr)
    sys.exit(1)
print("OK: companion status reports running=false on cold boot")
'

# ── 2. start → exercise the route (real ngrok call is best-effort) ──
# We do NOT require this to succeed (sandbox has no ngrok auth token),
# but we DO require the daemon route to respond with structured JSON
# rather than 404 / connection-refused / 5xx.
echo "→ GET /cli/companion/start"
start_status="$(curl -s -o /tmp/_k2so_companion_start.$$ \
    -w "%{http_code}" --connect-timeout 3 --max-time 20 \
    "${base}/cli/companion/start?${qs}")"
start_body="$(cat /tmp/_k2so_companion_start.$$)"
rm -f /tmp/_k2so_companion_start.$$
echo "  HTTP $start_status — body: $start_body"
if [ "$start_status" = "404" ]; then
    echo "FAIL: /cli/companion/start returned 404 — route not wired" >&2
    exit 1
fi
# 200 (started) or 400 (start failed — e.g. no ngrok token) are both
# acceptable; both prove the route dispatches.
if [ "$start_status" != "200" ] && [ "$start_status" != "400" ]; then
    echo "FAIL: /cli/companion/start returned unexpected status $start_status" >&2
    exit 1
fi
# Response must be JSON.
echo "$start_body" | python3 -c 'import json, sys; json.loads(sys.stdin.read())' || {
    echo "FAIL: /cli/companion/start did not return JSON" >&2
    exit 1
}

# ── 3. stop → idempotent ─────────────────────────────────────────────
echo "→ GET /cli/companion/stop"
stop_body="$(curl -sf --connect-timeout 3 --max-time 10 \
    "${base}/cli/companion/stop?${qs}")"
echo "  body: $stop_body"
echo "$stop_body" | python3 -c '
import json, sys
data = json.loads(sys.stdin.read())
if not data.get("ok"):
    print(f"FAIL: stop should return ok=true (idempotent), got {data}", file=sys.stderr)
    sys.exit(1)
print("OK: companion stop returns ok=true")
'

# ── 4. CLI surface — confirm `cmd_daemon_companion` exists in cli/k2so ──
# We can't exec the CLI directly against the sandbox daemon (it reads
# $HOME/.k2so/daemon.port), but we CAN confirm the dispatch arm is
# present so a regression that drops it again is caught here.
project_root="$(cd "$SCRIPT_DIR/../.." && pwd)"
cli_file="$project_root/cli/k2so"
if ! grep -q "companion)" "$cli_file" || ! grep -q "cmd_daemon_companion" "$cli_file"; then
    echo "FAIL: cli/k2so missing 'companion)' dispatch arm or cmd_daemon_companion() function" >&2
    exit 1
fi
# And the dispatch must route to cmd_daemon_companion (not fail_deprecated).
if ! grep -q "companion) *shift 2; *cmd_daemon_companion" "$cli_file"; then
    echo "FAIL: 'companion' case in cmd_daemon dispatcher does not route to cmd_daemon_companion" >&2
    exit 1
fi

echo "OK: daemon companion subcommand wired (routes + CLI dispatch)"
