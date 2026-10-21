#!/usr/bin/env bash
# Smoke r6 polish: `k2 msg <ws> "text"` to a workspace that isn't a
# connection prints the daemon's "not connected" teaching ONCE (it used to
# print it twice: the raw JSON on stdout and the teaching again on stderr)
# and still exits 3. Same for `--inbox-wake` / `--inbox-silent`.
#
# No real daemon: a stub HTTP server answers 403 not_connected. HOME is
# sandboxed. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    if [ "$2" = "$3" ]; then echo "  PASS: $1"; pass=$((pass + 1))
    else echo "  FAIL: $1 (got=$(printf %q "$2") want=$(printf %q "$3"))" >&2; fail=$((fail + 1)); fi
}

WORK="$(mktemp -d -t k2-msg-notconn-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/ws"
printf 'hello\n' >"$WORK/ws/note.md"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

HINT = "'beta' is not a local connection — add it with `k2 connections add beta` (or ask your human: Settings → Connections). Same-workspace is always allowed."

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        raw = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _handle(self):
        u = urlsplit(self.path)
        if u.path in ("/cli/workspace/msg", "/cli/inbox/compose", "/cli/inbox/deliver"):
            self._send(403, {"ok": False, "error": {"code": "not_connected", "hint": HINT}})
        else:
            self._send(200, {})

    def do_GET(self):
        self._handle()

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(n)
        self._handle()

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo "$PORT" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
PORT="$PORT" python3 "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 40); do
    if curl -s -o /dev/null --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/x"; then up=1; break; fi
    sleep 0.05
done
[ "$up" = "1" ] || { echo "FAIL: stub server did not start" >&2; exit 1; }

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2SO_PORT -u K2SO_HOOK_TOKEN -u K2SO_PROJECT_PATH \
        -u K2_CELL -u K2_PRIMARY -u K2_SESSION_ID -u K2_PANE_ID -u K2_TAB_ID \
        HOME="$HOME" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN="fake-token" \
        K2_PROJECT_PATH="$WORK/ws" \
        "$K2_CLI" "$@"
}

check_once() {
    local label="$1"; shift
    set +e
    run_k2 "$@" >"$WORK/out" 2>"$WORK/err"
    local rc=$?
    set -e
    local both
    both="$(cat "$WORK/out" "$WORK/err")"
    assert_eq "$label: exit 3" "$rc" "3"
    assert_eq "$label: the teaching once (stdout + stderr)" "$(printf '%s\n' "$both" | grep -o 'is not a local connection' | wc -l | tr -d ' ')" "1"
    assert_eq "$label: the code once" "$(printf '%s\n' "$both" | grep -o 'not_connected' | wc -l | tr -d ' ')" "1"
}

echo "== live msg to a workspace that isn't a connection =="
check_once "live msg" msg beta --from tester "ping"

echo "== --inbox-silent / --inbox-wake to it =="
check_once "inbox-silent" msg beta --inbox-silent --title "Note" --body "body"
check_once "inbox-wake" msg beta --inbox-wake --title "Note" --body "body"

echo
echo "msg_not_connected_once: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
