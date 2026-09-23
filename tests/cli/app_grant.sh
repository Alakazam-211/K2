#!/usr/bin/env bash
# k2 app grant list|add|rm — owner CLI for skin.db grants.
# Stub daemon only. Never touch real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*)
            echo "  PASS: $label"
            pass=$((pass + 1))
            ;;
        *)
            echo "  FAIL: $label (missing $(printf %q "$needle"))" >&2
            fail=$((fail + 1))
            ;;
    esac
}
assert_not_contains() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*)
            echo "  FAIL: $label (found $(printf %q "$needle"))" >&2
            fail=$((fail + 1))
            ;;
        *)
            echo "  PASS: $label"
            pass=$((pass + 1))
            ;;
    esac
}
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

SANDBOX="$(mktemp -d -t k2-app-grant-XXXXXX)"
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
echo "1" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
export K2_HOST=127.0.0.1
export K2_PORT=1
export K2_HOOK_TOKEN="fake-token"
unset K2_HOOK_SOCK K2SO_HOOK_SOCK K2SO_PORT K2SO_HOOK_TOKEN || true

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
        HOME="$HOME" \
        K2_HOST="${K2_HOST}" \
        K2_PORT="${K2_PORT}" \
        K2_HOOK_TOKEN="${K2_HOOK_TOKEN}" \
        "$K2_CLI" "$@"
}

echo "== bash -n =="
if bash -n "$K2_CLI"; then
    echo "  PASS: bash -n cli/k2"
    pass=$((pass + 1))
else
    echo "  FAIL: bash -n cli/k2" >&2
    fail=$((fail + 1))
fi

echo "== k2 app grant --help is the app family, not k2 people =="
set +e
help_out="$(run_k2 app grant --help 2>&1)"
help_rc=$?
set -e
assert_eq "k2 app grant --help exit 0" "$help_rc" "0"
assert_contains "grant help list" "$help_out" "k2 app grant list"
assert_contains "grant help add" "$help_out" "k2 app grant add"
assert_contains "grant help rm" "$help_out" "k2 app grant rm"
assert_contains "grant help subject" "$help_out" "--subject"
assert_contains "grant help kind" "$help_out" "--kind"
assert_contains "grant help target" "$help_out" "--target"
assert_contains "grant help role" "$help_out" "--role"
assert_contains "grant help scope" "$help_out" "--scope"
assert_not_contains "grant help is not people" "$help_out" "k2 people"
assert_not_contains "grant help is not /cli/people" "$help_out" "/cli/people"
schema="$(cat "$K2_CLI")"
assert_contains "schema grant add" "$schema" '"name": "app grant add"'
assert_contains "grant route" "$schema" "/cli/skin/grants"

echo "== missing flags fail before the daemon =="
set +e
missing="$(run_k2 app grant add --kind app 2>&1)"
missing_rc=$?
set -e
assert_eq "add missing flags exit 1" "$missing_rc" "1"
assert_contains "add usage" "$missing" "k2 app grant add"

echo "== list add rm against a stub daemon =="
STUB_PY="$SANDBOX/stub.py"
cat >"$STUB_PY" <<'PY'
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
import os

seen = os.environ["SEEN"]

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        raw = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _read(self):
        n = int(self.headers.get("Content-Length") or "0")
        raw = self.rfile.read(n)
        return json.loads(raw.decode() or "{}")

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/cli/skin/grants":
            self._send(200, json.dumps({"grants": [{
                "id": "g1",
                "subjectKind": "principal",
                "subjectId": "p-ada",
                "kind": "app",
                "targetId": "booking",
                "roleId": "r-guest",
                "scope": None,
            }]}))
            return
        self._send(404, json.dumps({"error": "not found"}))

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        body = self._read()
        with open(seen, "a") as f:
            f.write(path + "\n" + json.dumps(body) + "\n")
        if path == "/cli/skin/grants":
            self._send(200, json.dumps({
                "id": "g-new",
                "subjectKind": body.get("subjectKind"),
                "subjectId": body.get("subjectId"),
                "kind": body.get("kind"),
                "targetId": body.get("targetId"),
                "roleId": body.get("roleId"),
                "scope": body.get("scope"),
            }))
            return
        if path == "/cli/skin/grants/delete":
            self._send(200, json.dumps({"success": True}))
            return
        self._send(404, json.dumps({"error": "not found"}))

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY
STUB_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
export K2_PORT="$STUB_PORT"
echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"
SEEN="$SANDBOX/seen.txt"
: >"$SEEN"
PORT="$STUB_PORT" SEEN="$SEEN" python3 "$STUB_PY" &
STUB_PID=$!
for _i in 1 2 3 4 5 6 7 8 9 10; do
    if curl -sf --connect-timeout 1 "http://127.0.0.1:${STUB_PORT}/cli/skin/grants" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

set +e
list_out="$(run_k2 app grant list 2>/dev/null)"
list_rc=$?
add_out="$(run_k2 app grant add --subject-kind principal --subject p-ada --kind app --target booking --role guest 2>/dev/null)"
add_rc=$?
rm_out="$(run_k2 app grant rm --subject-kind workspace --subject ws-1 --kind mailbox --target inbox 2>/dev/null)"
rm_rc=$?
set -e
assert_eq "grant list exit 0" "$list_rc" "0"
assert_contains "list prints grant" "$list_out" "g1"
assert_contains "list prints subject" "$list_out" "principal p-ada"
assert_contains "list prints kind" "$list_out" "app booking"
assert_eq "grant add exit 0" "$add_rc" "0"
assert_contains "add prints id" "$add_out" "g-new"
assert_eq "grant rm exit 0" "$rm_rc" "0"
assert_contains "rm removed" "$rm_out" "Grant removed."

seen="$(cat "$SEEN")"
assert_contains "add posts grants" "$seen" "/cli/skin/grants"
assert_contains "add body subject" "$seen" '"subjectKind": "principal"'
assert_contains "add body subject id" "$seen" '"subjectId": "p-ada"'
assert_contains "add body kind" "$seen" '"kind": "app"'
assert_contains "add body target" "$seen" '"targetId": "booking"'
assert_contains "add body role" "$seen" '"roleId": "guest"'
assert_contains "rm posts delete" "$seen" "/cli/skin/grants/delete"
assert_contains "rm body workspace" "$seen" '"subjectKind": "workspace"'
assert_contains "rm body kind" "$seen" '"kind": "mailbox"'
assert_contains "rm body target" "$seen" '"targetId": "inbox"'

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
