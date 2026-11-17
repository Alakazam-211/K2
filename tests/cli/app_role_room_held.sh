#!/usr/bin/env bash
# What `k2 app role room <role> <room>` prints, and its exit code, for each
# answer the daemon gives. A stub daemon answers by role name:
#   applied  done at once (an App's agent with "Let this agent manage its
#            Apps' users" on, on a role of its own App): exit 0
#   held     one held request with a card for the owner (202): exit 2, and it
#            says so (it used to print an empty role and exit 0)
#   refused  owner_only: exit 3 with the daemon's next step
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is unset, no real
# daemon, no ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; diff <(printf '%s\n' "$3") <(printf '%s\n' "$2") >&2 || true; fi; }

SANDBOX="$(mktemp -d /tmp/k2rrh-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

ANSWERS = {
    "applied": (200, {"name": "applied", "roomAccess": [{"handle": "ops", "caps": ["thread:read"]}]}),
    "held": (202, {"ok": True, "status": "held", "kind": "manage",
                   "request": {"id": "w_9", "summary": "role held of sales/shop: add the room ops (another workspace's) with thread:read"},
                   "hint": "held for the owner: the owner sees it as a card on your Thread (and in Settings → App Approvals)."}),
    "refused": (403, {"error": {"code": "owner_only", "hint": "role 'refused' is a box-wide role: ask your human"}}),
}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        sent = json.loads(self.rfile.read(n) or b"{}")
        with open(sys.argv[2], "a") as f:
            f.write(json.dumps(sent, sort_keys=True) + "\n")
        status, answer = ANSWERS.get(sent.get("name", ""), (404, {"error": {"code": "not_found", "hint": "no role"}}))
        body = json.dumps(answer).encode()
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
python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" "$SANDBOX/sent" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"
export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
cd "$SANDBOX/ws"

# run <args…>: the agent's call; stdout+stderr in $out, the exit code in $rc.
run() {
    set +e
    out="$(K2_PORT="$PORT" K2_HOOK_TOKEN=passport-rrh "$K2" "$@" </dev/null 2>&1)"
    rc=$?
    set -e
}

run app role room applied ops --caps thread:read
expect_eq "applied: exit 0" "$rc" "0"
expect_eq "applied: prints the role" "$out" "Skin role applied
  ops  thread:read"
expect_eq "applied: sends the role, room and caps" "$(tail -n 1 "$SANDBOX/sent")" '{"caps": ["thread:read"], "handle": "ops", "name": "applied"}'

run app role room held ops --caps thread:read
expect_eq "held: exit 2" "$rc" "2"
expect_eq "held: says it waits for the owner, and where" "$out" "Held for the owner (request w_9): role held of sales/shop: add the room ops (another workspace's) with thread:read
  held for the owner: the owner sees it as a card on your Thread (and in Settings → App Approvals)."

run app role room refused ops
expect_eq "refused: exit 3" "$rc" "3"
case "$out" in
    *"Error: role 'refused' is a box-wide role: ask your human"*) ok "refused: the daemon's next step" ;;
    *) bad "refused: $out" ;;
esac

echo
echo "app_role_room_held: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
