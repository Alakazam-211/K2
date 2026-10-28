#!/usr/bin/env bash
# k2 front — requests and rendering against a stub daemon (front443, 0.46.1).
#
# status/enable/disable hit the right routes with the right method and
# body (`--mail` → {"mail":true}); a refusal body ({ok:false,error,message})
# or no daemon prints ONE plain stderr line and exits 1, never a traceback;
# `--json` passes the body through. Fake values only.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_absent() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-front-cli-XXXXXX)"
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
MODE_FILE="$WORK/mode"
LOG_FILE="$WORK/requests"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer

MODE_FILE, LOG_FILE = os.environ["MODE_FILE"], os.environ["LOG_FILE"]
STATUS = {
    "enabled": True, "mail": False, "serving": True,
    "socket": {"source": "systemd", "local": "[::]:443"},
    "restartNeeded": False, "cap": 16384, "e2eCap": 16384, "nofileSoftLimit": 524288,
    "connections": {"accepted": 12, "overCap": 0, "live": 2,
                    "routed": {"turn": 9, "caddy": 1, "stalwart": 0},
                    "closed": {"unknownSni": 2, "noSni": 0}},
    "routes": [{"host": "turn-box.k2.dev", "kind": "turn", "backend": "127.0.0.1:5349", "proxyV2": True}],
    "caddy": None, "helper": None,
}

class H(BaseHTTPRequestHandler):
    def _send(self, code, obj):
        data = (obj if isinstance(obj, str) else json.dumps(obj)).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _answer(self, method, body):
        path = self.path.split("?")[0]
        with open(LOG_FILE, "a") as f:
            f.write("%s %s %s\n" % (method, path, body))
        mode = open(MODE_FILE).read().strip()
        if mode == "refused":
            return self._send(409, {"ok": False, "error": "helper_needed", "message": "needs k2-root-helper protocol 8"})
        if mode == "agent":
            return self._send(403, {"ok": False, "error": {"code": "owner_only", "hint": "The owner runs: k2 front enable [--mail]"},
                                    "permission": {"requestable": False, "where": "the owner or an admin does this"}})
        if path == "/cli/front/status":
            return self._send(200, STATUS)
        if path == "/cli/front/enable":
            mail = json.loads(body or "{}").get("mail")
            return self._send(200, {"ok": True, "enabled": True, "mail": mail, "restarting": True,
                                    "warnings": [], "next": "the daemon restarts once"})
        if path == "/cli/front/disable":
            return self._send(200, {"ok": True, "enabled": False, "warnings": ["Stalwart is still on 127.0.0.1:8443: x"]})
        return self._send(404, '{"error":"route not found"}')

    def do_GET(self):
        self._answer("GET", "")

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._answer("POST", self.rfile.read(n).decode())

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo ok >"$MODE_FILE"
: >"$LOG_FILE"
MODE_FILE="$MODE_FILE" LOG_FILE="$LOG_FILE" PORT="$PORT" python3 "$WORK/stub.py" &
STUB_PID=$!
for _i in $(seq 1 50); do
    curl -sf --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/front/status" >/dev/null 2>&1 && break
    sleep 0.1
done
: >"$LOG_FILE"

k2() { env HOME="$HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token "$K2_CLI" "$@"; }
run() {
    set +e
    out="$(k2 "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
last_request() { tail -n 1 "$LOG_FILE"; }

echo "== status =="
run front status
assert_eq "status: exit 0" "$rc" "0"
assert_contains "status: request" "$(last_request)" "GET /cli/front/status"
assert_contains "status: on + serving" "$out" "front      : on · serving (socket from systemd)"
assert_contains "status: route line" "$out" "route      : turn-box.k2.dev -> turn 127.0.0.1:5349 PROXY v2"
assert_contains "status: cap" "$out" "cap        : 16384"
assert_contains "status: closed counters" "$out" "unknownSni 2"
run front
assert_eq "bare front = status" "$rc" "0"

echo "== enable / enable --mail / disable =="
run front enable
assert_eq "enable: exit 0" "$rc" "0"
assert_eq "enable: POST, mail false" "$(last_request)" 'POST /cli/front/enable {"mail":false}'
assert_contains "enable: restarting" "$out" "restarting : K2 will restart its daemon once to take port 443"
run front enable --mail
assert_eq "enable --mail: body" "$(last_request)" 'POST /cli/front/enable {"mail":true}'
assert_contains "enable --mail: mail line" "$out" "mail       : behind the front"
run front disable
assert_eq "disable: exit 0" "$rc" "0"
assert_eq "disable: POST" "$(last_request)" 'POST /cli/front/disable {}'
assert_contains "disable: warning shown" "$out" "warning    : Stalwart is still on 127.0.0.1:8443"

echo "== refusals and bad usage =="
echo refused >"$MODE_FILE"
for sub in status enable disable; do
    run front "$sub"
    assert_eq "$sub refused: exit 1" "$rc" "1"
    assert_contains "$sub refused: one plain line" "$err" "k2 front: error: helper_needed: needs k2-root-helper protocol 8"
    assert_absent "$sub refused: no traceback" "$err" "Traceback"
done
run front enable --json
assert_eq "--json refused: exit 1" "$rc" "1"
assert_contains "--json refused: body verbatim" "$out" '"error": "helper_needed"'
echo "== an agent: owner_only is exit 3 with the closing line =="
echo agent >"$MODE_FILE"
for sub in status enable disable; do
    run front "$sub"
    assert_eq "$sub agent: exit 3" "$rc" "3"
    assert_contains "$sub agent: owner_only + command" "$err" "k2 front: error: owner_only: The owner runs: k2 front enable"
    assert_contains "$sub agent: closing line" "$err" "It can't be requested."
    assert_absent "$sub agent: not a token error" "$err" "invalid or missing token"
done
run front enable --json
assert_eq "--json agent: exit 3" "$rc" "3"
echo refused >"$MODE_FILE"
run front disable --mail
assert_eq "disable --mail: usage exit 2" "$rc" "2"
run front enable --force
assert_eq "unknown flag: exit 2" "$rc" "2"
run front reload
assert_eq "unknown verb: exit 2" "$rc" "2"

echo "== no daemon =="
kill "$STUB_PID" 2>/dev/null || true
wait "$STUB_PID" 2>/dev/null || true
STUB_PID=""
run front status
assert_eq "no daemon: exit 1" "$rc" "1"
assert_contains "no daemon: unreachable" "$err" "daemon unreachable"

echo
echo "front_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
