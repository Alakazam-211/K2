#!/usr/bin/env bash
# k2 daemon status shows the k2-ptyd lines `k2 daemon survive status`
# shows (0.46.6): whether agents survive restarts, and how many came back
# at the last restart. Read from the local daemon's /boot-status (full view
# on loopback, no token), here a stub on a temp port. A daemon that says
# nothing about ptyd, or doesn't answer, adds no line. Never touches the
# real ~/.k2 or the running daemon (HOME is a temp dir; status only reads).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_absent() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-daemon-status-ptyd-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT
for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

export HOME="$WORK/home"
mkdir -p "$HOME/.k2"
BODY_FILE="$WORK/body"

cat >"$WORK/stub.py" <<'PY'
import os
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        data = open(os.environ["BODY_FILE"], "rb").read()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *_a):
        pass
HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
printf '%s' '{"phase":"ready","version":"0.0.0-test","ptyd":"on","adopted":2}' >"$BODY_FILE"
BODY_FILE="$BODY_FILE" PORT="$PORT" python3 "$WORK/stub.py" &
STUB_PID=$!
for _i in $(seq 1 50); do
    curl -sf --connect-timeout 1 "http://127.0.0.1:${PORT}/boot-status" >/dev/null 2>&1 && break
    sleep 0.1
done
printf '%s\n' "$PORT" >"$HOME/.k2/daemon.port"

run() { out="$(env HOME="$HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token "$K2_CLI" daemon status "$@" 2>&1)" || { bad "k2 daemon status $* exited non-zero: $out"; out=""; }; }

echo "== ptyd on, 2 adopted =="
run
assert_contains "status: survive line" "$out" "Agents survive: on  (k2-ptyd; k2 daemon survive status)"
assert_contains "status: came-back count" "$out" "Came back at the last restart: 2"
run --json
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["ptyd"]=="on" and d["adopted"]=="2", d' "$out" \
    && ok "--json: ptyd and adopted" || bad "--json: ptyd and adopted ($out)"

echo "== ptyd off with a reason =="
printf '%s' '{"phase":"ready","ptyd":"off (killmode_control-group)","adopted":0}' >"$BODY_FILE"
run
assert_contains "status: off reason" "$out" "Agents survive: off (killmode_control-group)"
assert_contains "status: zero came back" "$out" "Came back at the last restart: 0"

echo "== no ptyd field (switch off, nothing adopted) =="
printf '%s' '{"phase":"ready","version":"0.0.0-test"}' >"$BODY_FILE"
run
assert_absent "status: no survive line" "$out" "Agents survive"
run --json
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["ptyd"]=="" and d["adopted"]=="", d' "$out" \
    && ok "--json: empty ptyd fields" || bad "--json: empty ptyd fields ($out)"

echo "== not JSON =="
printf '%s' '<html>nope</html>' >"$BODY_FILE"
run
assert_absent "status: garbage adds no line" "$out" "Agents survive"
assert_absent "status: no traceback" "$out" "Traceback"

echo "== no daemon answering =="
kill "$STUB_PID" 2>/dev/null || true
wait "$STUB_PID" 2>/dev/null || true
STUB_PID=""
run
assert_absent "status: unreachable adds no line" "$out" "Agents survive"
assert_contains "status: still prints the header" "$out" "K2 Daemon"

echo
echo "daemon_status_ptyd: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
