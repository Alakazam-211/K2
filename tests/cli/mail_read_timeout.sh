#!/usr/bin/env bash
# Loud harness: `k2 mail read` socket timeout is code `timeout`, not
# daemon_unreachable / "is it running?". Connection refused stays
# daemon_unreachable. No live daemon. cargo does not cover _mail_py.
# Run with: bash tests/cli/mail_read_timeout.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -f "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found" >&2; exit 1; }
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not executable" >&2; exit 1; }

pass=0
fail=0

assert_exit() {
    local label="$1" expected="$2" got="$3"
    if [ "$got" = "$expected" ]; then
        echo "  PASS: $label (exit $got)"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label expected exit $expected got $got" >&2
        fail=$((fail + 1))
    fi
}

assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq "$needle"; then
        echo "  PASS: $label contains '$needle'"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label missing '$needle' in: $hay" >&2
        fail=$((fail + 1))
    fi
}

assert_absent() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq "$needle"; then
        echo "  FAIL: $label unexpected '$needle' in: $hay" >&2
        fail=$((fail + 1))
    else
        echo "  PASS: $label no '$needle'"
        pass=$((pass + 1))
    fi
}

WORKDIR="$(mktemp -d -t k2-mail-read-timeout-XXXXXX)"
trap 'kill $HANG_PID 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT

# Hanging daemon: accept HTTP, never write a response.
python3 - "$WORKDIR" <<'PY' &
import os, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

root = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return
    def do_GET(self):
        time.sleep(30)
    def do_POST(self):
        time.sleep(30)

httpd = HTTPServer(("127.0.0.1", 0), H)
port = httpd.server_address[1]
with open(os.path.join(root, "hang.port"), "w") as f:
    f.write(str(port))
httpd.serve_forever()
PY
HANG_PID=$!

for _ in $(seq 1 50); do
    [ -f "$WORKDIR/hang.port" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/hang.port" ] || { echo "FAIL: hang server never bound" >&2; exit 1; }
HANG_PORT="$(cat "$WORKDIR/hang.port")"

K2_HANG=(env K2_PORT="$HANG_PORT" K2_HOOK_TOKEN=fake K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR" K2_MAIL_HTTP_TIMEOUT=1)

echo "== mail read against a hung daemon is timeout, not daemon_unreachable =="
set +e
out="$("${K2_HANG[@]}" "$K2_CLI" mail read m_test123 --json 2>&1)"
rc=$?
set -e
assert_exit "hung read exit" 1 "$rc"
assert_contains "hung read code" "$out" '"code":"timeout"'
assert_contains "hung read hint" "$out" "still running"
assert_contains "hung read hint secs" "$out" "within 1s"
assert_absent "hung read not unreachable" "$out" "daemon_unreachable"
assert_absent "hung read not 'is it running'" "$out" "is it running?"

# Closed port: connect fails immediately.
python3 -c '
import socket, sys
s = socket.socket()
s.bind(("127.0.0.1", 0))
port = s.getsockname()[1]
s.close()
open(sys.argv[1], "w").write(str(port))
' "$WORKDIR/closed.port"
CLOSED_PORT="$(cat "$WORKDIR/closed.port")"

K2_DOWN=(env K2_PORT="$CLOSED_PORT" K2_HOOK_TOKEN=fake K2_PROJECT_PATH="$WORKDIR" HOME="$WORKDIR" K2_MAIL_HTTP_TIMEOUT=1)

echo "== mail read against connection-refused is still daemon_unreachable =="
set +e
out="$("${K2_DOWN[@]}" "$K2_CLI" mail read m_test123 --json 2>&1)"
rc=$?
set -e
assert_exit "refused read exit" 1 "$rc"
assert_contains "refused read code" "$out" '"code":"daemon_unreachable"'
assert_contains "refused read hint" "$out" "is it running?"
assert_absent "refused read not timeout" "$out" '"code":"timeout"'

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
