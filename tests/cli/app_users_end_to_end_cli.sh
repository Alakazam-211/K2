#!/usr/bin/env bash
# An agent manages its App's users (0.46.8), the CLI half: what the commands
# send and print.
#
# - k2 app user add --temp-password asks K2 for a password, prints it once,
#   and sends no password of its own; never both flags.
# - k2 app user sessions lists and ends sessions on the new App-user routes.
# - k2 app handover status --detail names the card and what moves.
# - k2 app chat names the current step, Sew's last answer and a blocked row.
# - k2 explain knows outside_app_rooms.
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is unset, the daemon
# is a stub on loopback. No real daemon, no ~/.k2, no keychain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; diff <(printf '%s\n' "$3") <(printf '%s\n' "$2") >&2 || true; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_no() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2aue-XXXXXX)"
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

LOG = sys.argv[2]
HANDOVER = {"handovers": [
    {"id": "hov-1", "state": "held", "appId": "shop", "fromHandle": "sales", "toHandle": "ops", "requestId": "req-9",
     "plan": {"lines": ["Hand over shop from sales to ops.", "Rooms you approved for shop's guests (desk) stay with the App."]}},
    {"id": "hov-0", "state": "offered", "appId": "crm", "fromHandle": "sales", "toHandle": "ops"},
]}
CHAT = {"ok": True, "name": "shop", "chat": {"appId": "svc-1", "feature": True, "key": None, "manifestChannels": 0, "spaces": [
    {"spaceId": "5101", "governance": "app", "governanceConfirmed": None, "channels": [], "people": [], "leaving": [], "waiting": None,
     "sync": [
        {"id": "b1", "op": "space_settings", "subject": "space:5101", "state": "blocked", "attempts": 3, "lastStatus": 409, "lastError": "app_mismatch: bound elsewhere", "updatedAt": 1},
        {"id": "d1", "op": "space_settings", "subject": "space:5101", "state": "done", "attempts": 1, "lastStatus": 200, "lastError": None, "updatedAt": 1},
     ]}]}}
ANSWERS = {
    "/cli/skin/users": {"username": "lee", "generatedPassword": "Temp-Pass-123", "passwordMustChange": True,
        "waitingForReset": {"rooms": ["ops"], "ask": "k2 permission request apps.user-password-reset user=wes app=shop mode=link --why x"}},
    "/cli/skin/users/sessions": {"ok": True, "username": "lee", "app": "svc-1", "sessions": [
        {"id": "ses_1", "kind": "browser", "client": "Safari on macOS", "createdAt": 1, "lastSeenAt": 2, "push": False}]},
    "/cli/skin/users/sessions/revoke": {"ok": True, "username": "lee", "app": "svc-1", "ended": 1},
    "/cli/app/handover/status": HANDOVER,
    "/cli/sew/app-chat": CHAT,
}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def answer(self, body_in):
        path = self.path.split("?", 1)[0]
        with open(LOG, "a") as f:
            f.write(json.dumps({"method": self.command, "path": path, "query": self.path.partition("?")[2], "body": body_in}) + "\n")
        v = ANSWERS.get(path)
        st = 200 if v is not None else 404
        body = json.dumps(v if v is not None else {"error": {"code": "not_found", "hint": path}}).encode()
        self.send_response(st)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        self.answer(None)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.answer(self.rfile.read(n).decode())

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" "$SANDBOX/log" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"
export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
cd "$SANDBOX/ws"

run() {
    set +e
    K2_PORT="$PORT" K2_HOOK_TOKEN=passport-aue "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
    RC=$?
    set -e
}
last_request() { tail -n 1 "$SANDBOX/log"; }

echo "== k2 app user add --temp-password =="
run app user add lee --app shop --temp-password
expect_eq "exit 0" "$RC" "0"
req="$(last_request)"
expect_has "asks K2 to make it" "$req" '\"generate\": true'
expect_no "sends no password" "$req" 'password'
out="$(cat "$SANDBOX/out")"
expect_has "the App user" "$out" "App user added: lee (App shop only; no box login)"
expect_has "the password, once" "$out" "Temporary password (shown once, K2 keeps no copy): Temp-Pass-123"
expect_has "what to do with it" "$out" "At their first sign-in K2 asks them to choose their own."
expect_has "how to hand it over" "$out" "privately"
: > "$SANDBOX/log"
run app user add lee --app shop --temp-password --password-stdin
expect_eq "not both: exit 2" "$RC" "2"
expect_has "not both: says so" "$(cat "$SANDBOX/err")" "--password-stdin or --temp-password"
expect_eq "not both: nothing sent" "$(wc -c < "$SANDBOX/log" | tr -d ' ')" "0"

echo "== the waiting note and a gone flag =="
run app user rooms lee --app shop --agent ops --apply-tokens
expect_eq "--apply-tokens: exit 2" "$RC" "2"
expect_has "--apply-tokens: says why" "$(cat "$SANDBOX/err")" "next request"
run app user add wes --app shop
expect_has "add: what waits and the card" "$(cat "$SANDBOX/out")" "Waiting: everything else works now, but they can't use room ops until they choose their own password"
expect_has "add: the card line" "$(cat "$SANDBOX/out")" "k2 permission request apps.user-password-reset user=wes app=shop mode=link"

echo "== k2 app user sessions =="
run app user sessions lee --app shop
expect_eq "list: exit 0" "$RC" "0"
expect_has "list: the route" "$(last_request)" '"path": "/cli/skin/users/sessions"'
expect_has "list: the session" "$(cat "$SANDBOX/out")" "ses_1"
run app user sessions lee --app shop --revoke-all
expect_eq "revoke: exit 0" "$RC" "0"
req="$(last_request)"
expect_has "revoke: the route" "$req" '"path": "/cli/skin/users/sessions/revoke"'
expect_has "revoke: POST" "$req" '"method": "POST"'
expect_has "revoke: how many" "$(cat "$SANDBOX/out")" "Ended 1 session(s) of lee"
run app user sessions --help
expect_has "help: the App's own agent too" "$(cat "$SANDBOX/out" "$SANDBOX/err")" "the App's own agent"

echo "== k2 app handover status --detail =="
run app handover status shop
expect_eq "plain: exit 0" "$RC" "0"
expect_no "plain: no card" "$(cat "$SANDBOX/out")" "card:"
run app handover status shop --detail
expect_eq "detail: exit 0" "$RC" "0"
out="$(cat "$SANDBOX/out")"
expect_has "detail: the card" "$out" "card: req-9"
expect_has "detail: what moves" "$out" "    - Rooms you approved for shop's guests (desk) stay with the App."
expect_has "detail: a row with no card yet" "$out" "none yet"
expect_has "detail: a row with no list yet" "$out" "--dry-run"

echo "== k2 app chat =="
run app chat shop
expect_eq "exit 0" "$RC" "0"
out="$(cat "$SANDBOX/out")"
expect_has "the current step" "$out" "(sending to Sew)"
expect_has "Sew's last answer" "$out" "Sew answered 409: app_mismatch: bound elsewhere"
expect_has "what a block needs" "$out" "the owner retries it: k2 app grant hooks retry b1"
expect_has "the finished step" "$out" "done, 1 try(s). Sew answered 200"

echo "== k2 explain =="
out="$(K2_PORT=1 "$K2" explain outside_app_rooms --json)"
expect_has "explain: what" "$out" "guests-rooms grant"
expect_has "explain: the fix" "$out" "k2 app grant <app-id> --kind guests-rooms --room <room>"
out="$(K2_PORT=1 "$K2" explain skin_room --json)"
expect_has "explain skin_room: the tie" "$out" "waiting for reset (agent-tied)"

echo
echo "pass=$pass fail=$fail"
[ "$fail" -eq 0 ]
