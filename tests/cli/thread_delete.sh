#!/usr/bin/env bash
# `k2 thread delete <addr> <seq|id>` (thread-message-delete-v1, 0.46.11): a
# person deletes a message they sent. CLI only: the body it sends (a number
# is a seq, anything else an id), what it prints, --json, and the exit codes
# (0 deleted or already, 3 refused, 4 not found, 2 usage).
#
# Hermetic: HOME is a throwaway dir, K2_*/K2SO_* unset, the daemon is a tiny
# python stub on loopback. No real daemon, no ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

SANDBOX="$(mktemp -d -t k2-thread-delete-XXXXXX)"
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

LOG = sys.argv[1]

def answer(req):
    which = req.get("id", req.get("seq"))
    if which == "agent-try":
        return 403, {"ok": False, "error": {"code": "person_only", "hint": "only the person who sent it can delete it; an agent can't delete a person's message"}}
    if which == "theirs":
        return 403, {"ok": False, "error": {"code": "not_your_message", "hint": "only the person who sent it can delete it"}}
    if which == "nope":
        return 404, {"ok": False, "error": {"code": "not_found", "hint": "no message nope on this Thread"}}
    already = which == "m9"
    return 200, {"ok": True, "id": "m4" if which == 4 else which, "seq": 4 if which == 4 else 9, "deleted": True, "already": already,
                 "deletedAt": 1762700100, "conversation_id": "c1", "addr": req.get("addr")}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode() if n else ""
        if u.path == "/cli/thread/message/delete":
            req = json.loads(raw or "{}")
            with open(LOG, "a") as f:
                f.write("%s %s\n" % (u.path, json.dumps(req, sort_keys=True)))
            status, obj = answer(req)
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
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.log"
: > "$LOG"
python3 -I "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
export K2_PORT="$(cat "$SANDBOX/port")"
export K2_HOOK_TOKEN=test-token

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi
}
assert_contains() {
    if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi
}
run() {
    set +e
    out="$("$K2_CLI" "$@" 2>&1)"; rc=$?
    set -e
}

echo "== help, study, schema =="
assert_contains "help names delete" "$("$K2_CLI" thread --help 2>&1)" 'k2 thread delete <addr> <seq|id> [--json]'
assert_contains "study human: what the agent does" "$("$K2_CLI" study human 2>&1)" "DELETED MESSAGES"
assert_contains "schema: the verb" "$("$K2_CLI" --schema 2>&1)" 'k2 thread delete <addr> <seq|id> [--json]'

echo "== a number is a seq, anything else an id =="
: > "$LOG"
run thread delete ledger 4
assert_eq "deleted: exit 0" "$rc" "0"
assert_eq "deleted: one line" "$out" "[4] m4  deleted; the agent was told (never what it said)"
run thread delete ledger '#9'
run thread delete ledger m9
assert_eq "already: exit 0" "$rc" "0"
assert_eq "already: says so" "$out" "[9] m9  already deleted"
assert_eq "the bodies sent" "$(cat "$LOG")" "$(printf '%s\n%s\n%s' \
    '/cli/thread/message/delete {"addr": "ledger", "seq": 4}' \
    '/cli/thread/message/delete {"addr": "ledger", "seq": 9}' \
    '/cli/thread/message/delete {"addr": "ledger", "id": "m9"}')"

echo "== --json =="
run thread delete ledger 4 --json
assert_eq "json: exit 0" "$rc" "0"
assert_eq "json: the daemon's answer" "$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print(d["deleted"], d["seq"], d["already"])' "$out")" "True 4 False"

echo "== refused and not found =="
run thread delete ledger agent-try
assert_eq "an agent: exit 3" "$rc" "3"
assert_contains "an agent: says why" "$out" "person_only: only the person who sent it can delete it"
run thread delete ledger theirs
assert_eq "someone else's: exit 3" "$rc" "3"
assert_contains "someone else's: says why" "$out" "not_your_message"
run thread delete ledger agent-try --json
assert_eq "json refusal: exit 3" "$rc" "3"
assert_eq "json refusal: the error code" "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["error"]["code"])' "$out")" "person_only"
run thread delete ledger nope
assert_eq "not found: exit 4" "$rc" "4"

echo "== usage errors (no daemon reached) =="
: > "$LOG"
run thread delete
assert_eq "no address: exit 2" "$rc" "2"
run thread delete ledger
assert_eq "no message: exit 2" "$rc" "2"
assert_contains "no message: says what" "$out" "its number (the [N] in k2 thread <addr>) or its id"
run thread delete ledger 4 5
assert_eq "extra argument: exit 2" "$rc" "2"
run thread delete ledger 4 --why x
assert_eq "unknown flag: exit 2" "$rc" "2"
assert_eq "no request for a usage error" "$(cat "$LOG")" ""

echo
echo "thread_delete: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
