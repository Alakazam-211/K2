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
# remove and block go straight through (0); a person's invite says it goes
# out once connected; the friends list marks a request with a pairing code;
# accept --code carries the code; `k2 msg <postal address>` and postal send
# post to /cli/postal/send (exit 0 held, 3 not a friend or a secret in the
# text, 2 for a K2-only flag); `k2 msg -- <postal address> "-x"` (and with
# --help as the text, or flags before `--`) sends the text as typed; a value
# flag before the address is refused with the address-first form's line
# (--from, --summary, --no-wake then --from, --idempotency-key, --secret),
# never dropped; route sends its targets; link posts {link: true} to the
# settings route and says what came of it (no token, no web page; a
# failure names owner-link only as the by-hand fallback); owner-link reads a
# token from stdin; adopt says "not available yet"; postal send takes a
# label by the Postal plane's rule and refuses the rest (exit 2, no
# request). Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is
# unset, the daemon is a python stub.

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
        elif p == "/cli/postal/send":
            b = json.loads(body or b"{}")
            if b.get("to") == "stranger::beta.postal.bot":
                status, out = 403, {"ok": False, "error": {"code": "postal_not_friends", "hint": "stranger::beta.postal.bot isn't a Postal friend"}}
            elif "hunter2" in (b.get("text") or ""):
                status, out = 403, {"ok": False, "error": {"code": "postal_secret_in_text", "hint": "this text holds the value of a secret"}}
            else:
                out = {"success": True, "id": "01J0000000000000000000000A", "deliveryId": "m_AbCdEf0123", "badge": "p_3f9a0c41d27be6058a1c",
                       "to": b.get("to"), "status": "held", "reason": None, "via": "postal", "live": "not_yet"}
        elif p == "/cli/postal/route":
            b = json.loads(body or b"{}")
            out = {"ok": True, "friend": b.get("friend"), "route": None if b.get("clear") else {"targets": b.get("targets", "").split(), "wake": b.get("wake", True)}}
        elif p == "/cli/postal/owner-link":
            out = {"ok": True, "label": "acme", "linked": True}
        elif p == "/cli/postal/settings" and method == "POST" and json.loads(body or b"{}").get("link") is True:
            if mode() == "linkfail":
                status, out = 409, {"ok": False, "error": {"code": "unauthorized", "hint": "this server couldn't get its Postal approval token, so nobody here can accept friend requests yet: the Postal network didn't accept this server's K2 Connect account (401). Try again with Link now in Settings → General → Postal (or k2 postal link); if it keeps failing, the owner can link approvals by hand with k2 postal owner-link"}}
            else:
                out = {"ok": True, "enabled": True, "adminsMayAccept": False, "approvalsLinked": True,
                       "decideToken": {"state": "minted", "label": "acme", "hint": "Postal approvals are linked for this server"}}
        elif p == "/cli/postal/settings":
            out = {"ok": True, "enabled": True, "adminsMayAccept": False, "approvalsLinked": mode() != "unlinked"}
        elif p == "/cli/postal/decide":
            b = json.loads(body or b"{}")
            if b.get("decision") == "accept" and b.get("code"):
                out = {"ok": True, "friend": {"id": b.get("id"), "peer": "lab::gamma.postal.bot", "status": "pending_out"}, "friends": False,
                       "hint": "your side is confirmed; they become friends when the other side types your code"}
            elif b.get("decision") == "accept":
                out = dict(ASKED, hint="a person answers the card on your pinned Thread (the Owner, or an Admin when the Owner allows it); the answer doesn't wake you: k2 postal friends shows it")
            else:
                out = {"ok": True, "friend": {"id": b.get("id"), "peer": "spam::gamma.postal.bot", "status": "rejected"}}
        elif p in ("/cli/postal/remove", "/cli/postal/block"):
            out = {"ok": True, "friend": {"id": "f-2", "peer": "ops::beta.postal.bot", "status": "revoked" if p.endswith("remove") else "blocked"}}
        elif p == "/cli/postal/friends":
            out = {"ok": True, "state": "on", "workspace": {"id": "w", "handle": "scout"}, "handle": {"address": "scout::initech.postal.bot", "status": "local"},
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
assert_contains "help: msg sends Postal mail" "$out" 'k2 msg <address> "text" [--no-wake] [--json]'
assert_contains "help: accept types the other side's code" "$out" "accept <request id> --code <their code>"
assert_contains "help: live delivery waits" "$out" "live delivery to this server"
assert_contains "help: requests never wake you" "$out" "Incoming requests never wake you"

echo "== the three card steps =="
run postal enable --why "to pair with the build agent"
assert_eq "enable: asked exits 2" "$rc" "2"
assert_contains "enable: says a card is up" "$out" "Asked: a card is on your person's Thread (pr_7)."
assert_contains "enable: says how the answer comes" "$out" "the answer comes back to you as a [permission:] turn."
assert_eq "enable: the why goes" "$(last /cli/postal/enable)" 'POST {"why": "to pair with the build agent"}'
run postal claim scout::initech.postal.bot --why "so ops can reach me"
assert_eq "claim: asked exits 2" "$rc" "2"
assert_eq "claim: address and why" "$(last /cli/postal/claim)" 'POST {"address": "scout::initech.postal.bot", "why": "so ops can reach me"}'
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
assert_contains "friends: a request with a pairing code" "$out" "pairing code waiting"
assert_contains "friends: waiting requests show their id" "$out" "id f-9"

echo "== a person's invite =="
echo person > "$WORK/mode"
run postal invite ops::beta.postal.bot --why "x"
assert_eq "person invite: exit 0" "$rc" "0"
assert_contains "person invite: recorded until connected" "$out" "as soon as this server is connected"
run postal accept f-9 --code "123 456"
assert_eq "accept --code: exit 0" "$rc" "0"
assert_eq "accept --code: the code goes" "$(last /cli/postal/decide)" 'POST {"code": "123 456", "decision": "accept", "id": "f-9"}'
assert_contains "accept --code: says it waits for the other side" "$out" "Your side is confirmed"
printf 'k2c_pd_0123456789abcdef0123456789abcdef\n' > "$WORK/tok"
set +e; out="$(k2 postal owner-link --token-stdin < "$WORK/tok" 2>&1)"; rc=$?; set -e
assert_eq "owner-link --token-stdin: exit 0" "$rc" "0"
assert_contains "owner-link: linked" "$out" "Postal approvals linked for acme"
assert_not_contains "owner-link: the token is never printed" "$out" "k2c_pd_"

echo "== Link now (approvals) =="
run postal link
assert_eq "link: exit 0" "$rc" "0"
assert_eq "link: posts link to the settings route" "$(last /cli/postal/settings)" 'POST {"link": true}'
assert_contains "link: says approvals are linked" "$out" "Approvals: linked; people here can accept friend requests"
assert_not_contains "link: no token" "$out" "k2c_pd_"
assert_not_contains "link: no web page" "$out" "https://"
echo linkfail > "$WORK/mode"
run postal link
assert_eq "link that fails: exit 1" "$rc" "1"
assert_contains "link that fails: says why" "$out" "didn't accept this server's K2 Connect account"
assert_contains "link that fails: owner-link only as the by-hand fallback" "$out" "by hand with k2 postal owner-link"
echo unlinked > "$WORK/mode"
run postal settings
assert_eq "settings unlinked: exit 0" "$rc" "0"
assert_contains "settings unlinked: offers k2 postal link" "$out" "Approvals: not linked yet, so friend requests cannot be accepted here: the owner runs k2 postal link"
assert_not_contains "settings unlinked: never owner-link first" "$out" "owner-link"
echo person > "$WORK/mode"

echo "== off =="
echo off > "$WORK/mode"
run postal status
assert_eq "postal_off: a refusal, exit 3" "$rc" "3"
assert_contains "postal_off: the error" "$out" "postal_off"
assert_contains "postal_off: ends with the Ask-with line" "$out" "Ask with: k2 permission request postal.enable"
rm -f "$WORK/mode"

echo "== sending =="
run msg ops::beta.postal.bot "hello there" --no-wake
assert_eq "msg to a Postal friend: exit 0" "$rc" "0"
assert_eq "msg: the body the daemon reads" "$(last /cli/postal/send)" 'POST {"text": "hello there", "to": "ops::beta.postal.bot", "wake": false}'
assert_contains "msg: the status" "$out" "Postal mail to ops::beta.postal.bot: held"
assert_contains "msg: the badge" "$out" "Badge:    p_3f9a0c41d27be6058a1c"
run postal send ops::beta.postal.bot "second"
assert_eq "postal send: exit 0" "$rc" "0"
assert_eq "postal send: the same route" "$(last /cli/postal/send)" 'POST {"text": "second", "to": "ops::beta.postal.bot", "wake": true}'
run msg stranger::beta.postal.bot "hi"
assert_eq "not a friend: refused, exit 3" "$rc" "3"
assert_contains "not a friend: the code" "$out" "postal_not_friends"
run msg ops::beta.postal.bot "the password is hunter2"
assert_eq "a vault secret in the text: refused, exit 3" "$rc" "3"
before="$(nreq)"
run msg ops::beta.postal.bot "hi" --summary "x"
assert_eq "a K2-only flag: usage, exit 2" "$rc" "2"
assert_contains "a K2-only flag: says why" "$out" "takes only the text"
assert_eq "a refused flag sends nothing" "$(nreq)" "$before"

echo "== sending: the address after -- =="
# After `--` the text is sent as typed: a dash first, or --help, is text.
run msg -- ops::beta.postal.bot "-x"
assert_eq "msg -- <address> -x: exit 0" "$rc" "0"
assert_eq "msg -- <address> -x: the text as typed" "$(last /cli/postal/send)" 'POST {"text": "-x", "to": "ops::beta.postal.bot", "wake": true}'
run msg -- ops::beta.postal.bot "--help"
assert_eq "msg -- <address> --help: exit 0" "$rc" "0"
assert_eq "msg -- <address> --help: sent as text" "$(last /cli/postal/send)" 'POST {"text": "--help", "to": "ops::beta.postal.bot", "wake": true}'
assert_not_contains "msg -- <address> --help: no help page" "$out" "Usage:"
# The flags before `--` are read as without it.
run msg --no-wake --json -- ops::beta.postal.bot "-x"
assert_eq "msg --no-wake --json -- <address> -x: exit 0" "$rc" "0"
assert_eq "msg --no-wake --json -- <address> -x: no wake" "$(last /cli/postal/send)" 'POST {"text": "-x", "to": "ops::beta.postal.bot", "wake": false}'
assert_contains "msg --json -- <address>: the JSON answer" "$out" '"badge": "p_3f9a0c41d27be6058a1c"'

echo "== sending: a flag before the address =="
# msg's own loop reads the address when a value flag comes first. A flag
# Postal mail can't honour is refused there with the address-first form's
# line, exit 2, nothing sent; it is never dropped.
same_refusal() { # <name> <flag-first words> VS <address-first words>
    local name="$1" first=() second=() b
    shift
    while [ "$1" != "VS" ]; do first+=("$1"); shift; done
    shift; second=("$@")
    run "${second[@]}"; local want="$out" want_rc="$rc"
    b="$(nreq)"
    run "${first[@]}"
    assert_eq "$name: exit 2" "$rc" "2"
    assert_eq "$name: the address-first form also exits 2" "$want_rc" "2"
    assert_eq "$name: the address-first form's line" "$out" "$want"
    assert_eq "$name: nothing sent" "$(nreq)" "$b"
}
same_refusal "msg --from me <address> hi" msg --from me ops::beta.postal.bot hi VS msg ops::beta.postal.bot hi --from me
assert_contains "msg --from me <address> hi: says why" "$out" "--from is for messages to agents on this server; Postal mail to ops::beta.postal.bot takes only the text, --no-wake and --json."
same_refusal "msg --summary s <address> hi" msg --summary s ops::beta.postal.bot hi VS msg ops::beta.postal.bot hi --summary s
assert_contains "msg --summary s <address> hi: says why" "$out" "--summary is for messages to agents on this server"
same_refusal "msg --no-wake --from me <address> hi" msg --no-wake --from me ops::beta.postal.bot hi VS msg ops::beta.postal.bot hi --no-wake --from me
assert_contains "msg --no-wake --from me <address> hi: --from is the one refused" "$out" "--from is for messages"
same_refusal "msg --from me <address> -- -x" msg --from me ops::beta.postal.bot -- "-x" VS msg ops::beta.postal.bot --from me -- "-x"
same_refusal "msg --idempotency-key <key> <address> hi" msg --idempotency-key abcdefgh ops::beta.postal.bot hi VS msg ops::beta.postal.bot hi --idempotency-key abcdefgh
assert_contains "msg --idempotency-key <key> <address> hi: unknown for Postal mail" "$out" "unknown flag '--idempotency-key' for Postal mail"
same_refusal "msg --secret K <address> hi" msg --secret K ops::beta.postal.bot hi VS msg ops::beta.postal.bot hi --secret K
same_refusal "msg --from me <address> (no text)" msg --from me ops::beta.postal.bot VS msg ops::beta.postal.bot --from me
assert_contains "msg --from me <address> (no text): --from is refused first" "$out" "--from is for messages"

echo "== the label rule (the Postal plane's) =="
# A label is 1 to 63 of a-z 0-9 -, no hyphen first or last, no two in a
# row, not all digits, not reserved and not starting with turn-. z3 is a
# live label. The reserved names are the plane's list.
l63="$(printf 'a%.0s' $(seq 1 63))"
l64="${l63}a"
# native: postal send
for l in z3 a "$l63" k2n-lab acme-box; do
    run postal send "ops::$l.postal.bot" "hi"
    assert_eq "label ${l:0:12} (${#l} chars): sent, exit 0" "$rc" "0"
    assert_eq "label ${l:0:12}: the address the daemon reads" "$(last /cli/postal/send)" "POST {\"text\": \"hi\", \"to\": \"ops::$l.postal.bot\", \"wake\": true}"
done
run postal send "ops::Z3.postal.bot" "hi"
assert_eq "label Z3: letters in any case, exit 0" "$rc" "0"
before="$(nreq)"
for l in "$l64" -a a- a.b a--b 123 7 turn-x \
    www api app admin dashboard account billing k2 k2c connect mail smtp ns1 ns2 status help support blog docs \
    k2-relay-probe cert relay edge frps db db-staging supabase dl postal postalbot frp acme probe; do
    # native: postal send
    run postal send "ops::$l.postal.bot" "hi"
    assert_eq "label ${l:0:12} (${#l} chars): refused, exit 2" "$rc" "2"
    assert_contains "label ${l:0:12}: says why" "$out" "isn't a Postal address"
done
assert_eq "a refused label sends nothing" "$(nreq)" "$before"

echo "== routing =="
run postal route inbox+pointer thread:alice --no-wake
assert_eq "route: exit 0" "$rc" "0"
assert_eq "route: targets and wake" "$(last /cli/postal/route)" 'POST {"targets": "inbox+pointer thread:alice", "wake": false}'
assert_contains "route: says where it lands" "$out" "inbox+pointer + thread:alice, no wake"
run postal route --clear --friend ops::beta.postal.bot
assert_eq "route --clear: the friend" "$(last /cli/postal/route)" 'POST {"clear": true, "friend": "ops::beta.postal.bot"}'

echo "== not available yet =="
before="$(nreq)"
run postal adopt
assert_eq "adopt: exit 1" "$rc" "1"
assert_contains "adopt: says not available yet" "$out" "not available yet"
assert_eq "adopt sent nothing" "$(nreq)" "$before"

echo
echo "postal CLI: $pass passed, $fail failed"
[ "$fail" = 0 ]
