#!/usr/bin/env bash
# `k2 thread ask` prints the card's id and its number (`<id>  card #<seq>`):
# the number is how the answer line names the card later
# (`[thread:<addr>] chose "Go" on card #<seq> ("<prompt start>")`), so an
# agent with several cards open can match the answer to its question.
# The help says what that answer line looks like.
#
# Hermetic: HOME is a throwaway dir, K2_*/K2SO_* unset, the daemon is a tiny
# python stub on loopback. No real daemon, no ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

SANDBOX="$(mktemp -d -t k2-thread-ask-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
    hermetic_cli_cleanup
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        req = json.loads(self.rfile.read(n).decode() or "{}") if n else {}
        if u.path == "/cli/thread/ask":
            status, obj = 200, {"ok": True, "id": "card-abc", "seq": 11290, "kind": "choice", "status": "pending",
                                "prompt": req.get("prompt"), "options": ["Go", "Stop"], "conversation_id": "c1", "addr": req.get("addr")}
        else:
            status, obj = 404, {"ok": False, "error": "unexpected route"}
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
export K2_PORT="$(cat "$SANDBOX/port")"
export K2_HOOK_TOKEN=test-token
mkdir -p "$SANDBOX/ws/.k2"
export K2_PROJECT_PATH="$SANDBOX/ws"
cd "$SANDBOX/ws"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi
}
assert_contains() {
    if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3"))"; fi
}

echo "== ask prints the id and the card number =="
set +e
out="$("$K2_CLI" thread ask ledger "Land all three?" --options "Go; Stop" 2>/dev/null)"; rc=$?
set -e
assert_eq "ask: exit 0" "$rc" "0"
assert_eq "ask: id and number on one line" "$out" "card-abc  card #11290"

echo "== --json keeps the daemon's answer =="
set +e
out="$("$K2_CLI" thread ask ledger "Land all three?" --options "Go; Stop" --json 2>/dev/null)"; rc=$?
set -e
assert_eq "json: exit 0" "$rc" "0"
assert_eq "json: id and seq" "$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(d["id"], d["seq"])' "$out")" "card-abc 11290"

echo "== help: what the answer line looks like =="
help_out="$("$K2_CLI" thread --help 2>&1)"
assert_contains "help: ask prints the number" "$help_out" '<card-id>  card #<seq>'
assert_contains "help: the answer names the card" "$help_out" '[thread:<addr>] chose "<option>" on card #<seq> ("<start of the prompt>")'

echo
echo "thread_ask_card_number: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
