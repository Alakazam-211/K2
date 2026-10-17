#!/usr/bin/env bash
# 0.46.0 person names — `k2 users list` and `k2 connections list --users`
# show the real name beside the username (display only) and pass
# firstName / lastName / displayName through in --json. A local HTTP stub
# stands in for the daemon; never touches a real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*) echo "  PASS: $label"; pass=$((pass + 1)) ;;
        *) echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2; fail=$((fail + 1)) ;;
    esac
}
assert_line_matches() {
    local label="$1" hay="$2" regex="$3"
    if printf '%s\n' "$hay" | grep -Eq -- "$regex"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (no line matches $(printf %q "$regex") in $(printf %q "$hay"))" >&2
        fail=$((fail + 1))
    fi
}

SANDBOX="$(mktemp -d -t k2-people-names-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

export HOME="$SANDBOX"
mkdir -p "$HOME/.k2"
unset K2_HOOK_SOCK K2SO_HOOK_SOCK K2SO_PORT K2SO_HOOK_TOKEN K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL || true

STUB_PY="$SANDBOX/stub.py"
cat >"$STUB_PY" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer

USERS = {"users": [
    {"username": "z3thon", "createdAt": "2026-10-01T00:00:00+00:00", "disabled": False, "role": "owner",
     "wasViewer": False, "canClone": True, "firstName": "Rosson", "lastName": "Long", "displayName": "Rosson Long"},
    {"username": "anna", "createdAt": "2026-10-02T00:00:00+00:00", "disabled": False, "role": "member",
     "wasViewer": False, "canClone": True, "firstName": None, "lastName": None, "displayName": "anna"},
]}
CONNS = {"workspace": {"name": "k2", "path": "/w/k2", "id": "ws-1"}, "connections": [], "users": [
    {"username": "z3thon", "role": "owner", "disabled": False, "firstName": "Rosson", "lastName": "Long", "displayName": "Rosson Long"},
    {"username": "anna", "role": "member", "disabled": False, "firstName": None, "lastName": None, "displayName": "anna"},
]}

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/cli/users":
            body = json.dumps(USERS).encode()
        elif path == "/cli/connections":
            body = json.dumps(CONNS).encode()
        else:
            self.send_response(404)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_POST = do_GET

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY
STUB_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
export K2_HOST=127.0.0.1
export K2_PORT="$STUB_PORT"
export K2_HOOK_TOKEN="fake-token"
PORT="$STUB_PORT" python3 "$STUB_PY" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if curl -sf --connect-timeout 1 "http://127.0.0.1:${STUB_PORT}/cli/users" >/dev/null 2>&1; then
        up=1
        break
    fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: stub daemon never came up" >&2; exit 1; }

echo "== k2 users list (human) =="
out="$("$K2_CLI" users list)"
assert_line_matches "header has a NAME column" "$out" '^USERNAME +NAME +ROLE +DISABLED +CREATED$'
assert_line_matches "z3thon row shows the real name" "$out" '^z3thon +Rosson Long +owner +no '
assert_line_matches "nameless row shows -" "$out" '^anna +- +member +no '

echo "== k2 users list --json =="
json_out="$("$K2_CLI" users list --json)"
assert_eq "--json passes names through" \
    "$(printf '%s' "$json_out" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["ok"], [(u["username"], u["firstName"], u["lastName"], u["displayName"]) for u in d["users"]])')" \
    "True [('z3thon', 'Rosson', 'Long', 'Rosson Long'), ('anna', None, None, 'anna')]"

echo "== k2 users list rejects a stray flag =="
set +e
"$K2_CLI" users list --nope >/dev/null 2>"$SANDBOX/err"
rc=$?
set -e
assert_eq "users list --nope exits 2" "$rc" "2"
assert_contains "usage mentions --json" "$(cat "$SANDBOX/err")" "k2 users list [--json]"

echo "== k2 connections list --users =="
out="$("$K2_CLI" connections list --users)"
assert_line_matches "people header has NAME" "$out" '^USERNAME +NAME +ROLE +DISABLED$'
assert_line_matches "z3thon person row has the name" "$out" '^z3thon +Rosson Long +owner +no$'
assert_line_matches "anna person row has -" "$out" '^anna +- +member +no$'
json_out="$("$K2_CLI" connections list --users --json)"
assert_eq "connections --json users carry names" \
    "$(printf '%s' "$json_out" | python3 -c 'import json,sys; d=json.load(sys.stdin); print([(u["username"], u["firstName"], u["lastName"], u["displayName"]) for u in d["users"]])')" \
    "[('z3thon', 'Rosson', 'Long', 'Rosson Long'), ('anna', None, None, 'anna')]"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
