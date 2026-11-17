#!/usr/bin/env bash
# `k2 heartbeat list` shows a schedule K2 turned off as auto-off, with the
# reason, since when, and the command that turns it back on; a person's
# disable stays a plain "no". A schedule waiting for a missing WAKEUP.md
# shows the daemon's detail (since when, fires skipped) and stays "yes".
# Stub daemon over TCP; no live K2 daemon. Fail loud; no skip-if-missing.
# Run with: bash tests/cli/heartbeat_list_auto_disabled.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

[ -x "$K2" ] || { echo "FAIL: $K2 not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

pass=0
fail=0
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*) echo "  PASS: $label"; pass=$((pass + 1)) ;;
        *) echo "  FAIL: $label (missing $(printf %q "$needle") in:)" >&2; printf '%s\n' "$hay" >&2; fail=$((fail + 1)) ;;
    esac
}
assert_absent() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*) echo "  FAIL: $label (unexpected $(printf %q "$needle"))" >&2; fail=$((fail + 1)) ;;
        *) echo "  PASS: $label"; pass=$((pass + 1)) ;;
    esac
}

WORKDIR="$(mktemp -d)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORKDIR"
}
trap cleanup EXIT
mkdir -p "$WORKDIR/home" "$WORKDIR/project"

python3 - "$WORKDIR" << 'PY' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

root = sys.argv[1]
base = {"frequency": "hourly", "specJson": "{\"every_seconds\":1800}", "lastFired": "2026-11-10T04:30:00Z",
        "nextFireAt": None, "waitDetail": None, "waitSince": None, "disabledReason": None}
rows = [
    dict(base, name="old-off", enabled=False, disabledReason="wakeup_missing",
         waitReason="disabled:wakeup_missing", waitDetail=".k2/heartbeats/old-off/WAKEUP.md",
         waitSince="2026-11-10T04:31:00Z"),
    dict(base, name="tired", enabled=False, disabledReason="failures",
         waitReason="disabled:failures", waitDetail="5 consecutive failed fires",
         waitSince="2026-11-10T06:00:00Z"),
    dict(base, name="paused", enabled=False, waitReason="disabled:user"),
    dict(base, name="waits", enabled=True, waitReason="wakeup_waiting",
         nextFireAt="2026-11-10T05:00:00Z",
         waitDetail="waiting for WAKEUP.md since 2026-11-10T04:31:00Z, 3 fires skipped: WAKEUP.md not found at .k2/heartbeats/waits/WAKEUP.md. K2 retries every fire; it runs as soon as the file can be read"),
]

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        return
    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/cli/heartbeat/list":
            body = json.dumps(rows).encode()
        else:
            body = json.dumps({"error": {"code": "not_found", "hint": "stub " + path}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

httpd = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(root, "stub.port"), "w").write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do
    [ -f "$WORKDIR/stub.port" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/stub.port" ] || { echo "FAIL: stub daemon never bound" >&2; exit 1; }

# A clean env: only the stub daemon is reachable (no inherited K2_* socket).
out="$(env -i HOME="$WORKDIR/home" PATH="$PATH" K2_AIRGAP=1 K2_HOST=127.0.0.1 \
    K2_PORT="$(cat "$WORKDIR/stub.port")" K2_HOOK_TOKEN=fake \
    K2_HOOK_SOCK="$WORKDIR/not-a-socket.sock" K2_PROJECT_PATH="$WORKDIR/project" TZ=UTC \
    "$K2" heartbeat list)"

echo "== auto-disabled rows say so, with reason, time and the way back =="
old_line="$(printf '%s\n' "$out" | grep '^old-off ')"
assert_contains "old-off reads auto-off" "$old_line" "auto-off"
assert_contains "old-off reason" "$out" "auto-disabled by K2 (off since 2026-11-10 04:31): WAKEUP.md could not be read"
assert_contains "old-off way back" "$out" "k2 heartbeat enable old-off"
assert_contains "failures reason" "$out" "auto-disabled by K2 (off since 2026-11-10 06:00): its fires failed too many times in a row"
assert_contains "failures way back" "$out" "k2 heartbeat enable tired"

echo "== a person's disable is a plain no =="
paused_line="$(printf '%s\n' "$out" | grep '^paused ')"
assert_contains "paused reads no" "$paused_line" " no "
assert_absent "paused not auto-off" "$paused_line" "auto-off"
assert_absent "no enable hint for a person's disable" "$out" "k2 heartbeat enable paused"

echo "== waiting for WAKEUP.md stays on and says why =="
waits_line="$(printf '%s\n' "$out" | grep '^waits ')"
assert_contains "waits reads yes" "$waits_line" " yes "
assert_contains "waits detail" "$out" "    waiting for WAKEUP.md since 2026-11-10T04:31:00Z, 3 fires skipped"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
