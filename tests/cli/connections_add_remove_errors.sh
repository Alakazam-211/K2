#!/usr/bin/env bash
# `k2 connections add` and `k2 connections remove` print their errors on
# stderr and only their results on stdout. Exit codes stay as they were:
# 0 done, 1 an error or an answer that is not JSON, 3 refused by the
# agents-add-connections switch.
#
# Covers, for add and for remove:
#   1. Done: the result line on stdout, nothing on stderr, exit 0 (add also
#      says "Already connected" for a no-op, and its warning stays on stderr).
#   2. An error as a string and as {code, hint}: "Error: ..." on stderr,
#      nothing on stdout, exit 1.
#   3. An answer that is not JSON: "Error: daemon returned an unparseable
#      response" on stderr, nothing on stdout, exit 1.
#   4. Gated: the JSON error on stderr, nothing on stdout, exit 3.
#
# HTTP is a small Python stub that answers by the request's target. HOME
# is sandboxed; the CLI gets the stub's port and a test token in its env.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

pass=0
fail=0
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}

WORK="$(mktemp -d -t k2-connections-errors-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
GATED = {"code": "agents_create_connections_disabled",
         "hint": "this agent isn't allowed to create or remove connections. Settings has the switch."}

class H(BaseHTTPRequestHandler):
    def _go(self):
        length = int(self.headers.get("Content-Length", 0) or 0)
        if length:
            self.rfile.read(length)
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        action = (q.get("action") or [""])[0]
        target = (q.get("target") or [""])[0]
        code, body = 200, None
        if parsed.path != "/cli/connections":
            code, body = 404, {"error": "not found"}
        elif target == "acme":
            body = {"ok": True, "target": "acme"} if action == "add" else {"ok": True}
        elif target == "already":
            body = {"ok": True, "noop": True, "target": "already", "message": "same pair",
                    "warning": "the peer is offline"}
        elif target == "string-error":
            code, body = 403, {"error": "only the server's owner may do this"}
        elif target == "hint-error":
            code, body = 400, {"error": {"code": "not_found", "hint": "no workspace named hint-error"}}
        elif target == "gated":
            code, body = 403, {"error": GATED}
        elif target == "not-json":
            raw = b"<html>proxy page</html>"
        else:
            code, body = 500, {"error": "unexpected target"}
        if body is not None:
            raw = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    do_GET = _go
    do_POST = _go

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port.tmp"), "w") as f:
    f.write(str(srv.server_address[1]))
os.replace(os.path.join(work, "stub.port.tmp"), os.path.join(work, "stub.port"))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
[ -s "$WORK/stub.port" ] || { echo "FAIL: the stub never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

conn() {
    set +e
    env -i PATH="$PATH" HOME="$HOME" TMPDIR="${TMPDIR:-/tmp}" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" \
        K2_HOOK_TOKEN=test-owner-token K2_PROJECT_PATH="$WORK/proj" \
        "$K2_CLI" connections "$@" >"$WORK/out" 2>"$WORK/err"
    rc=$?
    set -e
}
out() { cat "$WORK/out"; }
err() { cat "$WORK/err"; }

for verb in add remove; do
    echo "== connections $verb =="
    conn "$verb" acme
    assert_eq "$verb done: exit 0" "$rc" "0"
    if [ "$verb" = add ]; then
        assert_eq "$verb done: the result on stdout" "$(out)" "Connected to acme"
    else
        assert_eq "$verb done: the result on stdout" "$(out)" "Disconnected."
    fi
    assert_eq "$verb done: nothing on stderr" "$(err)" ""

    conn "$verb" string-error
    assert_eq "$verb string error: exit 1" "$rc" "1"
    assert_eq "$verb string error: nothing on stdout" "$(out)" ""
    assert_eq "$verb string error: on stderr" "$(err)" "Error: only the server's owner may do this"

    conn "$verb" hint-error
    assert_eq "$verb hint error: exit 1" "$rc" "1"
    assert_eq "$verb hint error: nothing on stdout" "$(out)" ""
    assert_eq "$verb hint error: on stderr" "$(err)" "Error: no workspace named hint-error"

    conn "$verb" not-json
    assert_eq "$verb not JSON: exit 1" "$rc" "1"
    assert_eq "$verb not JSON: nothing on stdout" "$(out)" ""
    assert_eq "$verb not JSON: on stderr" "$(err)" "Error: daemon returned an unparseable response: '<html>proxy page</html>'"

    conn "$verb" gated
    assert_eq "$verb gated: exit 3" "$rc" "3"
    assert_eq "$verb gated: nothing on stdout" "$(out)" ""
    assert_eq "$verb gated: the JSON error on stderr" \
        "$(python3 -c 'import json, sys; print(json.loads(sys.stdin.read())["error"]["code"])' <"$WORK/err")" \
        "agents_create_connections_disabled"
done

echo "== connections add, a no-op with a warning =="
conn add already
assert_eq "no-op: exit 0" "$rc" "0"
assert_eq "no-op: the result on stdout" "$(out)" "Already connected to already (same pair)"
assert_eq "no-op: the warning on stderr" "$(err)" "warning: the peer is offline"

echo
echo "connections_add_remove_errors: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
