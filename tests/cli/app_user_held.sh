#!/usr/bin/env bash
# 0.46.14: what `k2 app user …` prints, and its exit code, when the daemon
# holds the change for the owner (a login of the agent's own App that talks in
# another workspace's room), and what an agent is told about asking while
# permission cards are on or off.
#
# A stub daemon answers:
#   POST /cli/skin/users*, /cli/skin/roles/assign  username "held"
#        → 202 held (a card for the owner); anything else → 200
#   POST /cli/skin/grants → 403 owner_only (an agent's `k2 app grant add`)
#   GET  /cli/permission/enabled → {"enabled": <$SANDBOX/cards == on>}
#
# Checked: held = exit 2 with "Held for the owner (request …)", the hint and
# (password) the after-approval step, never the password; not held = exit 0
# as before; cards off = `k2 app grant add`, `k2 study apps` and `k2 app
# --help` never name `k2 permission request` and say what the owner can do;
# cards on = they name the card.
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
has() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1: no '$3' in: $2" ;; esac; }
lacks() { case "$2" in *"$3"*) bad "$1: '$3' in: $2" ;; *) ok "$1" ;; esac; }

SANDBOX="$(mktemp -d /tmp/k2uhd-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

PORT_FILE, SENT, CARDS = sys.argv[1], sys.argv[2], sys.argv[3]

def held(summary, extra=None):
    v = {"ok": True, "status": "held", "kind": "manage",
         "request": {"id": "w_7", "summary": summary},
         "hint": "held for the owner: the owner sees it as a card on your Thread (and in Settings → App Approvals)."}
    v.update(extra or {})
    return 202, v

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def answer(self, status, v):
        body = json.dumps(v).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/cli/permission/enabled":
            on = os.path.exists(CARDS) and open(CARDS).read().strip() == "on"
            return self.answer(200, {"enabled": on})
        return self.answer(404, {"error": {"code": "not_found", "hint": "no"}})
    def do_POST(self):
        path = urlparse(self.path).path
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) or b"{}"
        with open(SENT, "a") as f:
            f.write(path + " " + raw.decode() + "\n")
        sent = json.loads(raw)
        user = sent.get("username", "")
        if path == "/cli/skin/grants":
            return self.answer(403, {"ok": False, "error": {"code": "owner_only", "hint": "requires owner/admin — ask your human"}})
        if user == "held":
            if path == "/cli/skin/users/password":
                return self.answer(*held("let the agent set a new password for the login held (App sales/shop; it talks in ops) once",
                    {"afterApproval": "After approval, set the password once within 24 h: k2 app user password held --password-stdin. The password you sent was not used or kept.", "passwordNotUsed": True}))
            return self.answer(*held("remove the login held (App sales/shop; it talks in ops)"))
        if path == "/cli/skin/users/remove":
            return self.answer(200, {"success": True})
        if path == "/cli/skin/roles/assign":
            return self.answer(200, {"username": user, "roleName": sent.get("role")})
        if path == "/cli/skin/users/password":
            return self.answer(200, {"username": user, "hasPassword": True})
        return self.answer(200, {"username": user})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(PORT_FILE, "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" "$SANDBOX/sent" "$SANDBOX/cards" &
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
    out="$(K2_PORT="$PORT" K2_HOOK_TOKEN=passport-uhd "$K2" "$@" </dev/null 2>&1)"
    rc=$?
    set -e
}
run_pw() {
    set +e
    out="$(printf 'S3cret-not-shown-9' | K2_PORT="$PORT" K2_HOOK_TOKEN=passport-uhd "$K2" "$@" 2>&1)"
    rc=$?
    set -e
}

echo "== held changes =="
for args in "app user remove held" "app user role held wide" "app user add held --app shop --role wide" "app user reset held --app shop"; do
    # shellcheck disable=SC2086
    run $args
    expect_eq "$args: exit 2" "$rc" "2"
    has "$args: says it waits, with the request" "$out" "Held for the owner (request w_7):"
    has "$args: says where the owner sees it" "$out" "Settings → App Approvals"
done

run_pw app user password held --password-stdin
expect_eq "password held: exit 2" "$rc" "2"
has "password held: the one set after approval" "$out" "After approval, set the password once within 24 h"
lacks "password held: never prints the password" "$out" "S3cret-not-shown-9"

echo "== not held: as before =="
run app user remove ann
expect_eq "remove: exit 0" "$rc" "0"
expect_eq "remove: prints as before" "$out" "Skin user removed."
run_pw app user password ann --password-stdin
expect_eq "password: exit 0" "$rc" "0"
expect_eq "password: prints as before" "$out" "Skin password set for ann"

echo "== permission cards off =="
echo off > "$SANDBOX/cards"
run app grant add --subject-kind principal --subject dana --kind app --target shop --role viewer
expect_eq "grant add refused: exit 3" "$rc" "3"
lacks "grant add, cards off: no unknown command" "$out" "k2 permission"
has "grant add, cards off: the owner's own path" "$out" "Settings → Apps → <the App> → Add a person"
has "grant add, cards off: where cards turn on" "$out" "Settings → Permission Cards"
run study apps
lacks "study apps, cards off: no unknown command" "$out" "k2 permission request"
has "study apps, cards off: the owner's own path" "$out" "Settings → Apps → <the App> → Add a person"
has "study apps: the held changes" "$out" "A LOGIN OF YOUR APP IN ANOTHER WORKSPACE'S ROOM"
run app --help
lacks "app --help, cards off: no unknown command" "$out" "k2 permission request"
lacks "app --help: no tag leaks" "$out" "#if-"
has "app --help, cards off: where cards turn on" "$out" "Settings → Permission Cards"

echo "== permission cards on =="
echo on > "$SANDBOX/cards"
run app grant add --subject-kind principal --subject dana --kind app --target shop --role viewer
expect_eq "grant add refused: exit 3" "$rc" "3"
lacks "grant add, cards on: no owner's-path line (the refusal's Ask with: stands)" "$out" "Settings → Permission Cards"
run study apps
has "study apps, cards on: names the card" "$out" "k2 permission request apps.user-grant-box"
lacks "study apps: no tag leaks" "$out" "#if-"
run app --help
has "app --help, cards on: names the card" "$out" "k2 permission request app.login-grant"
lacks "app --help, cards on: no tag leaks" "$out" "#if-"

echo
echo "app_user_held: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
