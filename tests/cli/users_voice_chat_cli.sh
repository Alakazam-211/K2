#!/usr/bin/env bash
# k2 users and the Voice & Chat only role (0.46.10,
# prd-voice-chat-only-members-v1 S5 + the reviewer shares): CLI shapes
# against a tiny stub daemon on loopback (python3 http.server). No real
# daemon, no ~/.k2 (HOME is a throwaway dir), fake passwords only.
#
# Covers: `users add --role voice-chat --share-agent … --share-garden …`
# sends role and shares IN the add body (one write, V5) and never calls
# set-role; shares without voice_chat are a usage error (exit 2, no
# request); owner/admin still add then set-role; `users set-role` posts the
# body and says whether it applied live; an Admin refusal exits 3;
# `users share` replaces (POST set-shares) or shows (GET /cli/users);
# `users list` prints the shares line and --json carries `shares`; the help
# names the role.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2vc-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

LOG = sys.argv[1]
SHARES = {"agents": [{"id": "p-1", "address": "demo", "name": "Demo"}], "gardens": [{"id": "g-1", "name": "Phone"}]}
NOTICE = "A shared agent's memory is shared: every login talks to the same session."

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def log(self, method, body):
        u = urlparse(self.path)
        with open(LOG, "a") as f:
            f.write(json.dumps({"method": method, "path": u.path, "body": body}) + "\n")
        return u
    def do_GET(self):
        u = self.log("GET", None)
        if u.path == "/cli/users":
            return self.reply(200, {"users": [
                {"username": "rev", "role": "voice_chat", "disabled": False, "createdAt": "2026-11-07T00:00:00Z",
                 "displayName": "rev", "mustChangePassword": False, "shares": SHARES},
                {"username": "mo", "role": "member", "disabled": False, "createdAt": "2026-11-07T00:00:00Z",
                 "displayName": "mo", "mustChangePassword": False, "shares": {"agents": [], "gardens": []}},
            ]})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        u = self.log("POST", body)
        if u.path == "/cli/users/add":
            out = {"success": True, "mustChangePassword": body.get("mustChangePassword", True), "role": body.get("role", "member"),
                   "fixedPassword": body.get("fixedPassword", False)}
            if body.get("role") == "voice_chat":
                out["shares"] = SHARES
                out["warnings"] = ["demo: connections to other agents"] if body.get("force") else []
                if body.get("shareAgents"):
                    out["notice"] = NOTICE
            return self.reply(200, out)
        if u.path == "/cli/users/set-role":
            if body.get("username") == "boss":
                return self.reply(403, {"error": "role_change_not_allowed",
                                        "message": "An Admin moves people only between Member and Voice & Chat only. Other role changes are the Owner's."})
            frm = "voice_chat" if body.get("username") == "rev" else "member"
            live = frm == "voice_chat" or body.get("role") == "voice_chat"
            return self.reply(200, {"success": True, "from": frm, "role": body.get("role"), "live": live})
        if u.path == "/cli/users/set-shares":
            if body.get("username") == "risky" and not body.get("force"):
                return self.reply(409, {"ok": False, "error": {"code": "agent_not_demo", "hint": "share only a dedicated demo agent",
                                                               "risks": ["ops: vault grants (2)", "ops: a mailbox"]}})
            out = {"success": True, "shares": SHARES, "warnings": ["ops: vault grants (2)"] if body.get("force") else []}
            if body.get("agents"):
                out["notice"] = NOTICE
            return self.reply(200, out)
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"

last() { tail -1 "$LOG" | python3 -c "import json,sys; v=json.load(sys.stdin); x=v[sys.argv[1]]; print(x if isinstance(x, str) else json.dumps(x, sort_keys=True))" "$1"; }
count() { wc -l < "$LOG" | tr -d ' '; }

echo "== add a reviewer: role and shares ride the add body =="
out="$(printf 'review-pass-1\n' | "$K2" users add rev --role voice-chat --share-agent demo --share-garden g-1 --password-stdin --keep-password 2>"$SANDBOX/m1err")"
expect_has "created" "$out" "Created user rev."
expect_has "M1: add prints the shared-context warning" "$(cat "$SANDBOX/m1err")" "WARNING: A shared agent's memory is shared"
expect_has "says what it can use" "$out" "Voice & Chat only: joins voice rooms; shared agents: demo; shared Gardens: Phone."
expect_eq "one request (no set-role)" "$(count)" "1"
expect_eq "add path" "$(last path)" "/cli/users/add"
body="$(last body)"
expect_has "role in the body" "$body" '"role": "voice_chat"'
expect_has "agents in the body" "$body" '"shareAgents": ["demo"]'
expect_has "gardens in the body" "$body" '"shareGardens": ["g-1"]'
expect_has "kept password" "$body" '"mustChangePassword": false'

echo "== shares need voice_chat: usage error, no request =="
n="$(count)"
set +e
err="$(printf 'x-pass-12345\n' | "$K2" users add mo --share-agent demo --password-stdin 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "exit 2" "$rc" "2"
expect_has "says why" "$err" "only for --role voice_chat"
expect_eq "no request" "$(count)" "$n"

echo "== owner/admin still add, then set-role =="
: > "$LOG"
printf 'adm-pass-12345\n' | "$K2" users add ada --role admin --password-stdin >/dev/null
expect_eq "two requests" "$(count)" "2"
expect_has "add has no role" "$(head -1 "$LOG")" '"path": "/cli/users/add"'
expect_not "no role in an admin add body" "$(head -1 "$LOG")" '"role"'
expect_eq "then set-role" "$(last path)" "/cli/users/set-role"

echo "== set-role =="
out="$("$K2" users set-role rev member)"
expect_eq "set-role path" "$(last path)" "/cli/users/set-role"
expect_eq "set-role body" "$(last body)" '{"role": "member", "username": "rev"}'
expect_has "live, no sign-out" "$out" "rev: voice_chat -> member (applied now, no sign-out)"
"$K2" users set-role mo voice-chat >/dev/null
expect_eq "voice-chat spelling sent as voice_chat" "$(last body)" '{"role": "voice_chat", "username": "mo"}'
set +e
err="$("$K2" users set-role boss admin 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "a refused change exits 3" "$rc" "3"
expect_has "the daemon's reason" "$err" "Other role changes are the Owner's."
set +e
"$K2" users set-role rev viewer >/dev/null 2>&1; rc=$?
set -e
expect_eq "unknown role is usage" "$rc" "2"

echo "== share =="
out="$("$K2" users share rev --agent demo --garden Phone 2>"$SANDBOX/m1err")"
expect_has "M1: share prints the shared-context warning" "$(cat "$SANDBOX/m1err")" "WARNING: A shared agent's memory is shared"
expect_eq "set-shares path" "$(last path)" "/cli/users/set-shares"
expect_eq "set-shares body" "$(last body)" '{"agents": ["demo"], "gardens": ["Phone"], "username": "rev"}'
expect_has "renders" "$out" "agents:  demo"
"$K2" users share rev --clear >/dev/null
expect_eq "clear sends empty lists" "$(last body)" '{"agents": [], "gardens": [], "username": "rev"}'
out="$("$K2" users share rev)"
expect_eq "show reads the list" "$(last path)" "/cli/users"
expect_has "show gardens" "$out" "gardens: Phone"
set +e
"$K2" users share mo >/dev/null 2>&1; rc=$?
set -e
expect_eq "a member has no shares" "$rc" "1"

echo "== list =="
out="$("$K2" users list)"
expect_has "role column" "$out" "voice_chat"
expect_has "shares line" "$out" "rev (Voice & Chat only) shares: agents demo; gardens Phone"
js="$("$K2" users list --json)"
expect_has "json shares" "$js" '"shares":{"agents":[{"id":"p-1","address":"demo","name":"Demo"}]'

echo "== security review: fixed password, sources, force =="
: > "$LOG"
set +e
out="$(printf 'review-pass-1\n' | "$K2" users add rev2 --role voice_chat --share-agent demo --share-source fleet --fixed-password --force --keep-password --password-stdin 2>"$SANDBOX/err")"; rc=$?
set -e
err="$(cat "$SANDBOX/err")"
expect_eq "forced add exits 0" "$rc" "0"
body="$(last body)"
expect_has "fixedPassword in the body" "$body" '"fixedPassword": true'
expect_has "sources in the body" "$body" '"shareSources": ["fleet"]'
expect_has "force in the body" "$body" '"force": true'
expect_has "never temporary" "$body" '"mustChangePassword": false'
expect_has "fixed said" "$out" "The password is fixed"
expect_has "the warning is printed" "$err" "WARNING: shared anyway (--force): demo: connections to other agents"
"$K2" users set-role rev member --fixed-password >/dev/null
expect_eq "set-role carries fixedPassword" "$(last body)" '{"fixedPassword": true, "role": "member", "username": "rev"}'
"$K2" users share rev --agent demo --source fleet >/dev/null
expect_eq "share sources" "$(last body)" '{"agents": ["demo"], "gardens": [], "sources": ["fleet"], "username": "rev"}'
set +e
err="$("$K2" users share risky --agent ops 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "a non-demo agent is refused" "$rc" "1"
expect_has "the reason" "$err" "share only a dedicated demo agent"
expect_has "each risk" "$err" "  - ops: a mailbox"
set +e
err="$("$K2" users share risky --agent ops --force 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "forced share exits 0" "$rc" "0"
expect_has "forced share warns" "$err" "WARNING: shared anyway (--force): ops: vault grants (2)"

echo "== help =="
help="$("$K2" users --help 2>&1)"
expect_has "help: role" "$help" "--role owner|admin|member|voice_chat"
expect_has "help: set-role" "$help" "k2 users set-role <username> owner|admin|member|voice_chat"
expect_has "help: share" "$help" "k2 users share <username>"
expect_has "help: demo agent only" "$help" "Share ONLY a dedicated"
expect_has "help: fixed password" "$help" "--keep-password --fixed-password"

echo
echo "users voice_chat CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
