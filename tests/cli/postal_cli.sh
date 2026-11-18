#!/usr/bin/env bash
# Postal (Beta, 0.46.16): `k2 postal` and `k2 msg <postal address>` against a
# stub daemon (the daemon's own rules are k2-daemon's postal_routes and
# permission::actions::postal tests).
#
# Covers: help (no daemon) teaches the three card steps and says what isn't
# available yet; each agent step that puts a card up prints "Asked: …" and
# exits 2, with the body the daemon reads (enable's why, claim's address and
# why, invite's to and why, accept's id); a refusal exits 3 and ends
# with the daemon's "Ask with:" line; postal_off is a refusal (3); reject,
# remove and block go straight through (0); a person's invite says
# "not sent yet"; the friends list marks a request with a pairing code;
# send, route and adopt say "not available yet" and send nothing;
# `k2 msg <postal address>` exits 2 and sends nothing. Hermetic: HOME is a
# throwaway dir, every K2_*/K2SO_* var is unset, the daemon is a python stub.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
assert_not_contains() { if <<<"$2" grep -Fq -- "$3"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-postal-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

HOME2="$WORK/home"
mkdir -p "$HOME2/.k2" "$WORK/ws"
echo "owner-disk-token-postal" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]

def mode():
    p = os.path.join(work, "mode")
    return open(p).read().strip() if os.path.exists(p) else "on"

ASKED = {"ok": True, "asked": True, "card": "c-1", "request": "pr_7", "addr": "scout", "waiting": False,
         "hint": "a person answers the card on your pinned Thread; the answer comes back to you as a [permission:] turn"}
OFF = {"ok": False, "state": "off", "error": {"code": "postal_off", "hint": "Postal is off on this server, and only the server's owner turns it on: k2 postal enable puts that card on your person's Thread"},
       "permission": {"action": "postal.enable", "params": {}, "approvers": "owner",
                      "command": "k2 permission request postal.enable --why \"<what you need it for>\""}}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        n = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(n) if n else b""
        parsed = urllib.parse.urlparse(self.path)
        p = parsed.path
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps({"method": method, "path": p, "body": body.decode("utf-8", "replace")}) + "\n")
        status, out = 200, {"ok": True}
        if p == "/boot-status":
            out = {"view": "full", "phase": "ready", "features": ["send-once"]}
        elif mode() == "off" and p.startswith("/cli/postal/") and p not in ("/cli/postal/enable", "/cli/postal/settings"):
            status, out = 403, OFF
        elif p in ("/cli/postal/enable", "/cli/postal/claim", "/cli/postal/invite"):
            b = json.loads(body or b"{}")
            if p == "/cli/postal/invite" and not b.get("why"):
                status, out = 400, {"ok": False, "error": {"code": "usage", "hint": "say why: --why \"what you want to do together\""}}
            elif p == "/cli/postal/invite" and mode() == "person":
                out = {"ok": True, "friend": {"id": "f-o", "peer": b.get("to"), "status": "pending_out"}, "sent": False, "network": "not_available_yet"}
            else:
                out = ASKED
        elif p == "/cli/postal/decide":
            b = json.loads(body or b"{}")
            if b.get("decision") == "accept":
                out = dict(ASKED, hint="a person answers the card on your pinned Thread (the Owner, or an Admin when the Owner allows it); the answer doesn't wake you: k2 postal friends shows it")
            else:
                out = {"ok": True, "friend": {"id": b.get("id"), "peer": "spam::gamma.postal.bot", "status": "rejected"}}
        elif p in ("/cli/postal/remove", "/cli/postal/block"):
            out = {"ok": True, "friend": {"id": "f-2", "peer": "ops::beta.postal.bot", "status": "revoked" if p.endswith("remove") else "blocked"}}
        elif p == "/cli/postal/friends":
            out = {"ok": True, "state": "on", "workspace": {"id": "w", "handle": "scout"}, "handle": {"address": "scout::acme.postal.bot", "status": "local"},
                   "friends": [{"id": "f-9", "peer": "lab::gamma.postal.bot", "status": "pending_in", "ownerName": "Dana", "codeWaiting": True, "updatedAt": 0},
                               {"id": "f-o", "peer": "ops::beta.postal.bot", "status": "pending_out", "updatedAt": 0}]}
        else:
            status, out = 404, {"ok": False, "error": {"code": "not_found", "hint": p}}
        raw = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
STUB_PORT="$(cat "$WORK/stub.port")"

k2() { (cd "$WORK/ws" && env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" "$@"); }
nreq() { if [ -f "$WORK/reqs.jsonl" ]; then grep -vc '"/boot-status"' "$WORK/reqs.jsonl" || true; else echo 0; fi; }
# The last request to PATH: its JSON body with keys sorted, workspace dropped.
last() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if json.loads(l)["path"] == sys.argv[2]]
b = json.loads(rows[-1]["body"] or "{}")
b.pop("workspace", None)
print(rows[-1]["method"], json.dumps(b, sort_keys=True))' "$WORK/reqs.jsonl" "$1"
}
run() { set +e; out="$(k2 "$@" 2>&1)"; rc=$?; set -e; }

echo "== help, no daemon =="
out="$(env HOME="$WORK/nohome" K2_PORT=1 "$K2_CLI" postal --help 2>&1)"
assert_contains "help: step 1 asks the owner" "$out" "k2 postal enable [--why"
assert_contains "help: step 3 invites with a why" "$out" 'k2 postal invite <address> --why "what you want to do together"'
assert_contains "help: asked is exit 2" "$out" 'Each prints "Asked: …" and exits 2'
assert_contains "help: not available yet" "$out" "k2 postal send | route | adopt"
assert_contains "help: requests never wake you" "$out" "Incoming requests never wake you"

echo "== the three card steps =="
run postal enable --why "to pair with the build agent"
assert_eq "enable: asked exits 2" "$rc" "2"
assert_contains "enable: says a card is up" "$out" "Asked: a card is on your person's Thread (pr_7)."
assert_contains "enable: says how the answer comes" "$out" "the answer comes back to you as a [permission:] turn."
assert_eq "enable: the why goes" "$(last /cli/postal/enable)" 'POST {"why": "to pair with the build agent"}'
run postal claim scout::acme.postal.bot --why "so ops can reach me"
assert_eq "claim: asked exits 2" "$rc" "2"
assert_eq "claim: address and why" "$(last /cli/postal/claim)" 'POST {"address": "scout::acme.postal.bot", "why": "so ops can reach me"}'
run postal invite ops::beta.postal.bot --why "to review each other's PRs"
assert_eq "invite: asked exits 2" "$rc" "2"
assert_eq "invite: to and why" "$(last /cli/postal/invite)" "POST {\"to\": \"ops::beta.postal.bot\", \"why\": \"to review each other's PRs\"}"
run postal invite ops::beta.postal.bot
assert_eq "invite without --why: usage, exit 2" "$rc" "2"
assert_contains "invite without --why: says why" "$out" "say why"

echo "== incoming requests =="
run postal accept f-9
assert_eq "accept: the card again, exit 2" "$rc" "2"
assert_contains "accept: the answer doesn't wake you" "$out" "the answer doesn't wake you"
assert_eq "accept: the request id" "$(last /cli/postal/decide)" 'POST {"decision": "accept", "id": "f-9"}'
run postal reject f-9
assert_eq "reject: the agent's own, exit 0" "$rc" "0"
assert_contains "reject: says so" "$out" "spam::gamma.postal.bot: rejected"
run postal block ops::beta.postal.bot
assert_eq "block: exit 0" "$rc" "0"
run postal friends
assert_contains "friends: a request with a pairing code" "$out" "has a pairing code"
assert_contains "friends: waiting requests show their id" "$out" "id f-9"

echo "== a person's invite =="
echo person > "$WORK/mode"
run postal invite ops::beta.postal.bot --why "x"
assert_eq "person invite: exit 0" "$rc" "0"
assert_contains "person invite: recorded, not sent" "$out" "Recorded, not sent yet"

echo "== off =="
echo off > "$WORK/mode"
run postal status
assert_eq "postal_off: a refusal, exit 3" "$rc" "3"
assert_contains "postal_off: the error" "$out" "postal_off"
assert_contains "postal_off: ends with the Ask-with line" "$out" "Ask with: k2 permission request postal.enable"
rm -f "$WORK/mode"

echo "== not available yet =="
before="$(nreq)"
for sub in send route adopt; do
    run postal "$sub" ops::beta.postal.bot
    assert_eq "$sub: exit 1" "$rc" "1"
    assert_contains "$sub: says not available yet" "$out" "not available yet"
done
run msg scout::acme.postal.bot "hello"
assert_eq "msg to a Postal address: exit 2" "$rc" "2"
assert_contains "msg to a Postal address: says it can't yet" "$out" "can't send Postal mail yet"
assert_eq "none of them sent anything" "$(nreq)" "$before"

echo
echo "postal CLI: $pass passed, $fail failed"
[ "$fail" = 0 ]
