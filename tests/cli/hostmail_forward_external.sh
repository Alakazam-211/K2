#!/usr/bin/env bash
# Outside forwards (prd-hostmail-external-forwarding-v1 §4.1, FW4): the CLI
# half. k2 hostmail forward set|show|unset|pause|resume|list, k2 hostmail
# person link, k2 mail forward (person routes).
#
# Hermetic: a throwaway HOME, every K2_*/K2SO_* var unset, and a python stub
# daemon on loopback that answers the forward routes with the daemon's JSON
# shapes (crates/k2-daemon/src/mail/forward.rs) and logs each request body.
# Checks:
#   * keep rides ONLY when --keep / --no-keep is given (outside forwards
#     default to keep:true daemon-side);
#   * the 202 pending_verify answer prints the confirm-link line;
#   * show prints the pause reason in words, the confirm link and 24 h counts;
#   * person_only, owner_only resume and destination_stopped refusals exit
#     with the right code and tell the caller what to do;
#   * the help has no "k2-forward" and no "not Gmail" (FW-A21);
#   * k2 mail forward is a verb now (no longer "moved to hostmail");
#   * an older mail server: set prints upgrade_required with the
#     `k2 hostmail upgrade` hint (exit 1), and show says the forward is held
#     ("not forwarding: the mail server needs upgrading").
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

fail() { echo "FAIL: $*" >&2; exit 1; }

SANDBOX="$(mktemp -d /tmp/k2fwd-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = sys.argv[1]

PAUSED = {
    "id": "f1", "address": "sales@brand.test", "to": "me@gmail.test", "kind": "external",
    "keep": True, "state": "paused", "pauseReason": "refused",
    "pauseWords": "the destination refused too many forwards as spam or by policy",
    "setBy": "agent:it", "createdAt": 1, "updatedAt": 2, "activatedAt": 2, "pausedAt": 3,
    "verify": {"state": "verified", "expiresAt": None, "verifiedAt": 1700000000},
    "last24h": {"forwarded": 12, "delivered": 9, "refused": 3, "refusedNoSuchUser": 0, "refusedPolicy": 3},
}
PENDING = dict(PAUSED, state="pending_verify", pauseReason=None, pauseWords=None,
               verify={"state": "pending", "expiresAt": 1700086400, "verifiedAt": None},
               last24h={"forwarded": 0, "delivered": 0, "refused": 0})

def reply(h, status, obj):
    body = json.dumps(obj).encode()
    h.send_response(status)
    h.send_header("Content-Type", "application/json")
    h.send_header("Content-Length", str(len(body)))
    h.send_header("Connection", "close")
    h.end_headers()
    h.wfile.write(body)

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def handle_any(self):
        u = urllib.parse.urlparse(self.path)
        q = dict(urllib.parse.parse_qsl(u.query))
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode() if n else ""
        body = json.loads(raw) if raw else {}
        with open(LOG, "a") as f:
            f.write("%s %s %s\n" % (self.command, u.path, raw))
        p = u.path
        if p.startswith("/cli/mail/self/") and q.get("token") == "agent-passport":
            return reply(self, 403, {"ok": False, "error": {"code": "person_only", "hint": "this is a person's own mailbox route"}})
        if p in ("/cli/mail/forward", "/cli/mail/self/forward") and self.command == "POST":
            to = body.get("to", "")
            addr = body.get("address") or "me@brand.test"
            if addr.startswith("oldbox@"):
                return reply(self, 409, {"ok": False, "error": {"code": "upgrade_required", "hint": "outside forwarding needs the mail server upgraded: k2 hostmail upgrade (same-box forwards work as they are)"}})
            if to.endswith("@gmail.test"):
                return reply(self, 202, {"ok": True, "address": addr, "to": to, "keep": body.get("keep", True),
                                         "kind": "external", "state": "pending_verify", "sentVia": "system",
                                         "message": "We emailed %s a confirm link." % to, "note": None})
            return reply(self, 200, {"ok": True, "address": addr, "to": to, "keep": body.get("keep", False),
                                     "kind": "internal", "state": "active", "note": None})
        if p in ("/cli/mail/forward", "/cli/mail/self/forward"):
            addr = q.get("address") or "me@brand.test"
            if addr.startswith("none@"):
                return reply(self, 200, {"ok": True, "address": addr, "to": None, "keep": False, "forward": None, "events": []})
            f = dict(PENDING if addr.startswith("pending@") else PAUSED, address=addr)
            if addr.startswith("oldbox@"):
                f.update(pauseReason="upgrade_required",
                         pauseWords="not forwarding: the mail server needs upgrading: k2 hostmail upgrade")
            return reply(self, 200, {"ok": True, "address": addr, "to": None, "keep": False, "forward": f,
                                     "events": [{"id": "e1", "forwardId": "f1", "event": "paused", "actor": "k2",
                                                 "detail": {"reason": "refused"}, "at": 1700000500}]})
        if p.endswith("/forward/resume"):
            addr = body.get("address", "")
            if addr.startswith("stopped@"):
                return reply(self, 409, {"ok": False, "error": {"code": "destination_stopped", "hint": "the destination pressed Stop in K2's email, so this forward can't be resumed; set it again to send a new confirm link"}})
            if addr.startswith("auto@"):
                return reply(self, 403, {"ok": False, "error": {"code": "owner_only", "hint": "K2 paused this forward because the destination refused too many forwards as spam or by policy; the owner, an Admin or the mailbox's person resumes it (k2 hostmail forward resume)"}})
            return reply(self, 200, {"ok": True, "address": addr, "state": "active", "pauseReason": None, "note": None})
        if p.endswith("/forward/pause"):
            return reply(self, 200, {"ok": True, "address": body.get("address", "me@brand.test"), "state": "paused", "pauseReason": "manual"})
        if p.endswith("/forward/unset"):
            return reply(self, 200, {"ok": True, "address": body.get("address", "me@brand.test"), "to": None, "removed": True, "id": "f1", "note": None})
        if p == "/cli/mail/forwards":
            return reply(self, 200, {"ok": True, "forwards": [PAUSED]})
        if p == "/cli/mail/address/person-link":
            return reply(self, 200, {"ok": True, "address": body.get("address"), "person": body.get("person")})
        return reply(self, 404, {"ok": False, "error": {"code": "not_found", "hint": "stub: " + p}})
    do_GET = handle_any
    do_POST = handle_any

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
[ -s "$SANDBOX/port" ] || fail "the stub daemon did not start"

export HOME="$SANDBOX/home"
mkdir -p "$HOME"
export K2_PORT="$(cat "$SANDBOX/port")"
export K2_HOOK_TOKEN=owner-token

last_body() { tail -n 1 "$LOG"; }

# ── help (FW-A21) ───────────────────────────────────────────────────────
for h in "hostmail forward --help" "hostmail forward set --help" "hostmail forward show --help" \
         "hostmail forward unset --help" "hostmail forward pause --help" "hostmail forward resume --help" \
         "hostmail forward list --help" "mail forward --help" "hostmail person link --help"; do
    # shellcheck disable=SC2086
    out="$("$K2" $h)"
    printf '%s' "$out" | grep -q 'USAGE' || fail "k2 $h must print leaf help: $out"
    if printf '%s' "$out" | grep -qi 'k2-forward\|not Gmail\|cannot be Gmail'; then
        fail "k2 $h still says k2-forward / not Gmail: $out"
    fi
done
"$K2" hostmail forward set --help | grep -q -- '--no-keep' || fail "forward set --help must teach --no-keep"
"$K2" hostmail forward resume --help | grep -q 'owner or an Admin only' || fail "resume --help must name the server brake rule"

# ── set: keep only when asked ───────────────────────────────────────────
out="$("$K2" hostmail forward set sales@brand.test --to me@gmail.test)"
last_body | grep -q '"keep"' && fail "set without --keep/--no-keep must not send keep: $(last_body)"
printf '%s' "$out" | grep -q "a confirm link was sent to me@gmail.test; the forward starts once it's clicked" \
    || fail "202 pending_verify must print the confirm-link line: $out"
printf '%s' "$out" | grep -q 'keeps a copy' || fail "outside default keep must show: $out"

"$K2" hostmail forward set sales@brand.test --to me@gmail.test --no-keep >/dev/null
last_body | grep -q '"keep":false' || fail "--no-keep must send keep:false: $(last_body)"
"$K2" hostmail forward set sales@brand.test --to other@brand.test --keep >/dev/null
last_body | grep -q '"keep":true' || fail "--keep must send keep:true: $(last_body)"
out="$("$K2" hostmail forward set sales@brand.test --to other@brand.test)"
printf '%s' "$out" | grep -q ': active' || fail "same-box set must print active: $out"

set +e
out="$("$K2" hostmail forward set sales@brand.test --to me@gmail.test --keep --no-keep 2>&1)"; rc=$?
set -e
[ "$rc" -eq 2 ] || fail "--keep with --no-keep must exit 2 (got $rc): $out"

# ── show: pause words, confirm link, 24 h counts ────────────────────────
out="$("$K2" hostmail forward show sales@brand.test)"
printf '%s' "$out" | grep -q 'state=paused  reason=refused' || fail "show must print state + reason: $out"
printf '%s' "$out" | grep -q 'paused because the destination refused too many forwards' || fail "show must print pauseWords: $out"
printf '%s' "$out" | grep -q 'last 24h: forwarded 12, delivered 9, refused 3' || fail "show must print the 24 h counts: $out"
printf '%s' "$out" | grep -q 'confirm link: clicked' || fail "show must print the confirm state: $out"
printf '%s' "$out" | grep -q 'paused' || fail "show must list events: $out"
out="$("$K2" hostmail forward show pending@brand.test)"
printf '%s' "$out" | grep -q 'confirm link: sent, expires' || fail "pending show must print the link expiry: $out"
out="$("$K2" hostmail forward show none@brand.test)"
printf '%s' "$out" | grep -q 'forward=none' || fail "no forward must print forward=none: $out"
"$K2" hostmail forward show sales@brand.test --json | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["forward"]["pauseReason"]=="refused", d' \
    || fail "show --json must carry the route JSON"

# ── an older mail server (upgrade_required) ─────────────────────────────
set +e
out="$("$K2" hostmail forward set oldbox@brand.test --to me@gmail.test 2>&1)"; rc=$?
set -e
[ "$rc" -eq 1 ] || fail "upgrade_required must exit 1 (got $rc): $out"
printf '%s' "$out" | grep -q 'upgrade_required' || fail "set must print the code: $out"
printf '%s' "$out" | grep -q 'k2 hostmail upgrade' || fail "set must print the upgrade hint: $out"
out="$("$K2" hostmail forward show oldbox@brand.test)"
printf '%s' "$out" | grep -q 'state=paused  reason=upgrade_required' || fail "held show must print the reason: $out"
printf '%s' "$out" | grep -q 'not forwarding: the mail server needs upgrading: k2 hostmail upgrade' || fail "held show must say it isn't forwarding: $out"
printf '%s' "$out" | grep -q 'resumes by itself after the upgrade' || fail "held show must say how it lifts: $out"
printf '%s' "$out" | grep -q 'paused because' && fail "held show must not print the generic pause line: $out"
"$K2" hostmail forward --help | grep -q 'upgrade_required' || fail "forward --help must name upgrade_required"
"$K2" hostmail forward set --help | grep -q 'upgrade_required' || fail "forward set --help must name upgrade_required"
"$K2" hostmail forward resume --help | grep -q 'upgrade_required' || fail "forward resume --help must name upgrade_required"
"$K2" study mail | grep -q 'upgrade_required' || fail "k2 study mail must name upgrade_required"

# ── pause / resume / unset / list ───────────────────────────────────────
"$K2" hostmail forward pause sales@brand.test | grep -q 'forward paused for sales@brand.test' || fail "pause output"
last_body | grep -q '^POST /cli/mail/forward/pause' || fail "pause must POST /cli/mail/forward/pause: $(last_body)"
"$K2" hostmail forward resume sales@brand.test | grep -q 'forward resumed for sales@brand.test' || fail "resume output"
"$K2" hostmail forward unset sales@brand.test | grep -q 'forward removed for sales@brand.test' || fail "unset output"
"$K2" hostmail forward list --paused --external | grep -q 'sales@brand.test  to=me@gmail.test' || fail "list output"
grep -q '^GET /cli/mail/forwards' "$LOG" || fail "list must GET /cli/mail/forwards"

# ── refusals ────────────────────────────────────────────────────────────
set +e
out="$("$K2" hostmail forward resume auto@brand.test 2>&1)"; rc=$?
set -e
[ "$rc" -eq 3 ] || fail "an agent resuming an automatic pause must exit 3 (got $rc): $out"
printf '%s' "$out" | grep -q "the owner, an Admin or the mailbox's person resumes it" || fail "owner_only resume must say who: $out"
printf '%s' "$out" | grep -q 'ask your human' && fail "owner_only resume must not add the generic tail: $out"

set +e
out="$("$K2" hostmail forward resume stopped@brand.test 2>&1)"; rc=$?
set -e
[ "$rc" -eq 1 ] || fail "destination_stopped must exit 1 (got $rc): $out"
printf '%s' "$out" | grep -q 'k2 hostmail forward set stopped@brand.test --to <dest>' || fail "destination_stopped must name the set command: $out"

set +e
out="$(K2_HOOK_TOKEN=agent-passport "$K2" mail forward show 2>&1)"; rc=$?
set -e
[ "$rc" -eq 3 ] || fail "person_only must exit 3 (got $rc): $out"
printf '%s' "$out" | grep -q 'k2 hostmail forward' || fail "person_only must point agents at k2 hostmail forward: $out"

# ── k2 mail forward (person routes) ─────────────────────────────────────
out="$("$K2" mail forward set --to me@gmail.test)"
last_body | grep -q '^POST /cli/mail/self/forward ' || fail "mail forward set must POST the person route: $(last_body)"
last_body | grep -q '"keep"' && fail "mail forward set without --keep must not send keep: $(last_body)"
last_body | grep -q '"address"' && fail "mail forward set without --address must not send address: $(last_body)"
printf '%s' "$out" | grep -q "a confirm link was sent to me@gmail.test" || fail "person set must print the confirm line: $out"
"$K2" mail forward show | grep -q 'state=paused' || fail "mail forward show"
"$K2" mail forward pause --address me@brand.test >/dev/null
last_body | grep -q '^POST /cli/mail/self/forward/pause .*"address":"me@brand.test"' || fail "mail forward pause --address: $(last_body)"
"$K2" mail forward resume >/dev/null
last_body | grep -q '^POST /cli/mail/self/forward/resume' || fail "mail forward resume route: $(last_body)"
"$K2" mail forward unset >/dev/null
last_body | grep -q '^POST /cli/mail/self/forward/unset' || fail "mail forward unset route: $(last_body)"

# ── person link ─────────────────────────────────────────────────────────
"$K2" hostmail person link sales@brand.test maria | grep -q 'is now linked to maria' || fail "person link output"
last_body | grep -q '"person":"maria"' || fail "person link body: $(last_body)"
"$K2" hostmail person link sales@brand.test --unlink | grep -q 'no longer linked' || fail "person unlink output"
last_body | grep -q '"person":null' || fail "person unlink must send null: $(last_body)"
set +e
out="$("$K2" hostmail person link sales@brand.test 2>&1)"; rc=$?
set -e
[ "$rc" -eq 2 ] || fail "person link without a login must exit 2 (got $rc): $out"

echo "PASS: hostmail forward external (CLI)"
