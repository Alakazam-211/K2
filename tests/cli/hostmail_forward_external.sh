#!/usr/bin/env bash
# Outside forwards (prd-hostmail-external-forwarding-v1 §4.1, FW4; opt-out and
# several destinations, Rosson 2026-11-02): the CLI half. k2 hostmail forward
# set|add|rm|show|unset|pause|resume|list, k2 hostmail person link, k2 mail
# forward (person routes).
#
# Hermetic: a throwaway HOME, every K2_*/K2SO_* var unset, and a python stub
# daemon on loopback that answers the forward routes with the daemon's JSON
# shapes (crates/k2-daemon/src/mail/forward.rs) and logs each request body.
# Checks:
#   * keep rides ONLY when --keep / --no-keep is given (outside forwards
#     default to keep:true daemon-side);
#   * --to repeats (a list on the wire), one --to stays a string;
#   * set prints the state and the daemon's message about the stop link;
#   * show prints every destination, the pause reason in words, the notice
#     and 24 h counts;
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
OLD = dict(PAUSED, state="pending_verify", pauseReason=None, pauseWords=None, notice="stale",
           noticeWords="not on: it was waiting for a confirm and was set more than 7 days ago, so K2 didn't turn it on. It sends nothing. To turn it on, set it again",
           verify={"state": "expired", "expiresAt": None, "verifiedAt": None},
           last24h={"forwarded": 0, "delivered": 0, "refused": 0})
TOLD = dict(PAUSED, id="f2", to="two@gmail.test", state="active", pauseReason=None, pauseWords=None,
            notice="sent", noticeAt=1700000000)

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
        if p == "/cli/mail/config/forward-new-per-day":
            if q.get("token") == "agent-passport" or self.headers.get("Authorization", "").endswith("agent-passport"):
                return reply(self, 403, {"ok": False, "error": {"code": "owner_only", "hint": "only the owner changes how many new outside addresses this server may add a day"},
                                         "permission": {"requestable": False, "where": "the owner does this, with k2 hostmail config --forward-new-per-day <n>"}})
            if body.get("perDay", 0) > 1000:
                return reply(self, 400, {"ok": False, "error": {"code": "usage", "hint": "--forward-new-per-day can be at most 1000, and %s is out of range. Nothing changed" % body.get("perDay")}})
            return reply(self, 200, {"ok": True, "perDay": body["perDay"], "default": 20, "max": 1000})
        if p.endswith("/forward/rm"):
            return reply(self, 200, {"ok": True, "address": body.get("address") or "me@brand.test",
                                     "removed": [body["to"]] if isinstance(body["to"], str) else body["to"],
                                     "remaining": ["left@gmail.test"], "note": None})
        if p.endswith("/forward/add") or (p in ("/cli/mail/forward", "/cli/mail/self/forward") and self.command == "POST"):
            to = body.get("to", "")
            addr = body.get("address") or "me@brand.test"
            if to == "stopped@gmail.test" and not body.get("restartStopped"):
                return reply(self, 409, {"ok": False, "error": {"code": "destination_stopped", "hint": "stopped@gmail.test pressed Stop in K2's email, so K2 won't forward to it. To override that, add --restart-stopped; ask them first"}})
            if to == "onetoomany@gmail.test":
                return reply(self, 409, {"ok": False, "error": {"code": "too_many_destinations", "hint": "a mailbox forwards to at most 100 addresses, and this would make 101"}})
            if to == "onemore@gmail.test":
                return reply(self, 429, {"ok": False, "error": {"code": "new_outside_limit", "hint": "this server adds at most 20 new outside addresses a day: 0 went on in the last 24 hours, and this would add 100. Nothing changed. Add fewer, try later, or forward to a mailbox on this server. Only the owner can raise the limit, up to 1000: k2 hostmail config --forward-new-per-day <n>"}})
            if isinstance(to, list):
                dests = [{"to": t, "keep": True, "kind": "external", "state": "active", "new": True, "notice": "sent", "sentVia": "system",
                          "message": "Forwarding to %s is on. K2 emailed %s to say so, with a link to stop it." % (t, t), "note": None} for t in to]
                return reply(self, 200, {"ok": True, "address": addr, "to": to[0], "keep": True, "kind": "external", "state": "active",
                                         "sentVia": "system", "message": dests[0]["message"], "note": None, "destinations": dests})
            if addr.startswith("oldbox@"):
                return reply(self, 409, {"ok": False, "error": {"code": "upgrade_required", "hint": "outside forwarding needs the mail server upgraded: k2 hostmail upgrade (same-box forwards work as they are)"}})
            if to.endswith("@gmail.test"):
                return reply(self, 200, {"ok": True, "address": addr, "to": to, "keep": body.get("keep", True),
                                         "kind": "external", "state": "active", "sentVia": "system",
                                         "message": "Forwarding to %s is on. K2 emailed %s to say so, with a link to stop it." % (to, to), "note": None})
            return reply(self, 200, {"ok": True, "address": addr, "to": to, "keep": body.get("keep", False),
                                     "kind": "internal", "state": "active", "note": None})
        if p in ("/cli/mail/forward", "/cli/mail/self/forward"):
            addr = q.get("address") or "me@brand.test"
            if addr.startswith("none@"):
                return reply(self, 200, {"ok": True, "address": addr, "to": None, "keep": False, "forward": None, "events": []})
            f = dict(OLD if addr.startswith("old@") else PAUSED, address=addr)
            if addr.startswith("cant@"):
                f.update(cantArrive="mail to me@gmail.test can't arrive: this server hosts its domain and has no such mailbox")
            if addr.startswith("oldbox@"):
                f.update(pauseReason="upgrade_required",
                         pauseWords="not forwarding: the mail server needs upgrading: k2 hostmail upgrade")
            rows = [f, dict(TOLD, address=addr)] if addr.startswith("two@") else [f]
            return reply(self, 200, {"ok": True, "address": addr, "to": None, "keep": False, "forward": f, "forwards": rows,
                                     "events": [{"id": "e1", "forwardId": "f1", "event": "paused", "actor": "k2",
                                                 "detail": {"reason": "refused"}, "at": 1700000500}]})
        if p.endswith("/forward/resume"):
            addr = body.get("address", "")
            if addr.startswith("stopped@"):
                return reply(self, 409, {"ok": False, "error": {"code": "destination_stopped", "hint": "that address pressed Stop in K2's email, so this forward can't be resumed; only the owner or an Admin can choose it again (k2 hostmail forward add <mailbox> --to <address> --restart-stopped)"}})
            if addr.startswith("auto@"):
                return reply(self, 403, {"ok": False, "error": {"code": "owner_only", "hint": "K2 paused this forward because the destination refused too many forwards as spam or by policy; the owner, an Admin or the mailbox's person resumes it (k2 hostmail forward resume)"}})
            rows = [{"to": body.get("to") or "me@gmail.test", "state": "active", "pauseReason": None}]
            skipped = [{"to": "gone@gmail.test", "why": "the owner resumes this one", "code": "owner_only"}] if addr.startswith("some@") else []
            return reply(self, 200, {"ok": True, "address": addr, "state": "active", "pauseReason": None, "forwards": rows, "skipped": skipped, "note": None})
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
for h in "hostmail forward --help" "hostmail forward set --help" "hostmail forward add --help" "hostmail forward rm --help" \
         "hostmail forward show --help" "hostmail forward unset --help" "hostmail forward pause --help" "hostmail forward resume --help" \
         "hostmail forward list --help" "mail forward --help" "hostmail person link --help"; do
    # shellcheck disable=SC2086
    out="$("$K2" $h)"
    printf '%s' "$out" | grep -q 'USAGE' || fail "k2 $h must print leaf help: $out"
    if printf '%s' "$out" | grep -qi 'k2-forward\|not Gmail\|cannot be Gmail'; then
        fail "k2 $h still says k2-forward / not Gmail: $out"
    fi
    if printf '%s' "$out" | grep -qi 'confirm link'; then
        fail "k2 $h still talks about a confirm link: $out"
    fi
done
"$K2" hostmail forward set --help | grep -q -- '--no-keep' || fail "forward set --help must teach --no-keep"
"$K2" hostmail forward resume --help | grep -q 'owner or an Admin only' || fail "resume --help must name the server brake rule"

# ── set: keep only when asked ───────────────────────────────────────────
out="$("$K2" hostmail forward set sales@brand.test --to me@gmail.test)"
last_body | grep -q '"keep"' && fail "set without --keep/--no-keep must not send keep: $(last_body)"
printf '%s' "$out" | grep -q "K2 emailed me@gmail.test to say so, with a link to stop it" \
    || fail "set must print the daemon's line about the stop link: $out"
printf '%s' "$out" | grep -q ': active' || fail "outside set must print active: $out"
last_body | grep -q '"to":"me@gmail.test"' || fail "one --to must stay a string on the wire: $(last_body)"
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

# ── several destinations ────────────────────────────────────────────────
out="$("$K2" hostmail forward set sales@brand.test --to a@gmail.test --to b@gmail.test)"
last_body | grep -q '"to":\["a@gmail.test","b@gmail.test"\]' || fail "repeated --to must be a list: $(last_body)"
printf '%s' "$out" | grep -q 'sales@brand.test -> a@gmail.test' || fail "set must print the first: $out"
printf '%s' "$out" | grep -q 'sales@brand.test -> b@gmail.test' || fail "set must print the second: $out"
"$K2" hostmail forward add sales@brand.test --to c@gmail.test --no-keep >/dev/null
last_body | grep -q '^POST /cli/mail/forward/add ' || fail "add must POST /cli/mail/forward/add: $(last_body)"
last_body | grep -q '"keep":false' || fail "add must carry --no-keep: $(last_body)"
out="$("$K2" hostmail forward rm sales@brand.test --to c@gmail.test)"
last_body | grep -q '^POST /cli/mail/forward/rm ' || fail "rm must POST /cli/mail/forward/rm: $(last_body)"
printf '%s' "$out" | grep -q 'no longer forwards to c@gmail.test' || fail "rm output: $out"
printf '%s' "$out" | grep -q 'still forwarding to: left@gmail.test' || fail "rm must name what's left: $out"
set +e
out="$("$K2" hostmail forward rm sales@brand.test 2>&1)"; rc=$?
set -e
[ "$rc" -eq 2 ] || fail "rm without --to must exit 2 (got $rc): $out"
set +e
out="$("$K2" hostmail forward rm sales@brand.test --to a@gmail.test --keep 2>&1)"; rc=$?
set -e
[ "$rc" -eq 2 ] || fail "rm --keep must exit 2 (got $rc): $out"
"$K2" hostmail forward pause sales@brand.test --to a@gmail.test >/dev/null
last_body | grep -q '"to":"a@gmail.test"' || fail "pause --to must send it: $(last_body)"
"$K2" hostmail forward resume sales@brand.test --to a@gmail.test >/dev/null
last_body | grep -q '"to":"a@gmail.test"' || fail "resume --to must send it: $(last_body)"
out="$("$K2" hostmail forward resume some@brand.test)"
printf '%s' "$out" | grep -q 'left alone: gone@gmail.test: the owner resumes this one' || fail "resume must say what it left: $out"
set +e
out="$("$K2" hostmail forward unset sales@brand.test --to a@gmail.test 2>&1)"; rc=$?
set -e
[ "$rc" -eq 2 ] || fail "unset takes no --to (rm does) (got $rc): $out"
out="$("$K2" hostmail forward show two@brand.test)"
printf '%s' "$out" | grep -q 'to=me@gmail.test' || fail "show must print the first destination: $out"
printf '%s' "$out" | grep -q 'to=two@gmail.test' || fail "show must print the second destination: $out"
printf '%s' "$out" | grep -q 'told two@gmail.test: emailed it a stop link' || fail "show must print the notice: $out"

# ── refusals that say what to do (exit 1) ───────────────────────────────
set +e
out="$("$K2" hostmail forward set sales@brand.test --to stopped@gmail.test 2>&1)"; rc=$?
set -e
[ "$rc" -eq 1 ] || fail "set to a stopped address must exit 1 (got $rc): $out"
printf '%s' "$out" | grep -q -- '--restart-stopped' || fail "destination_stopped must name the override: $out"
"$K2" hostmail forward set sales@brand.test --to stopped@gmail.test --restart-stopped >/dev/null
last_body | grep -q '"restartStopped":true' || fail "--restart-stopped must reach the daemon: $(last_body)"
for t in onetoomany@gmail.test onemore@gmail.test; do
    set +e
    out="$("$K2" hostmail forward add sales@brand.test --to "$t" 2>&1)"; rc=$?
    set -e
    [ "$rc" -eq 1 ] || fail "$t must exit 1 (got $rc): $out"
    printf '%s' "$out" | grep -q 'at most' || fail "$t must say the limit: $out"
done

# ── the daily cap (the owner only) ──────────────────────────────────────
out="$("$K2" hostmail config --forward-new-per-day 300)"
last_body | grep -q '^POST /cli/mail/config/forward-new-per-day .*"perDay": *300' || fail "the cap must POST its route: $(last_body)"
printf '%s' "$out" | grep -q 'may now add 300 new outside forward addresses a day (default 20, most 1000)' || fail "cap output: $out"
set +e
out="$("$K2" hostmail config --forward-new-per-day 5000 2>&1)"; rc=$?
set -e
[ "$rc" -eq 2 ] || fail "over the ceiling must exit 2 (got $rc): $out"
printf '%s' "$out" | grep -q 'at most 1000' || fail "over the ceiling must say the most: $out"
set +e
out="$("$K2" hostmail config --forward-new-per-day lots 2>&1)"; rc=$?
set -e
[ "$rc" -eq 2 ] || fail "a non-number must exit 2 (got $rc): $out"
set +e
out="$("$K2" hostmail config --forward-new-per-day 30 --address-cap 3 2>&1)"; rc=$?
set -e
[ "$rc" -eq 2 ] || fail "with other settings must exit 2 (got $rc): $out"
"$K2" hostmail config --help | grep -q 'THE OWNER ONLY' || fail "config --help must say who"
set +e
out="$(K2_HOOK_TOKEN=agent-passport "$K2" hostmail config --forward-new-per-day 1000 2>&1)"; rc=$?
set -e
[ "$rc" -eq 3 ] || fail "an agent raising the cap must exit 3 (got $rc): $out"
printf '%s' "$out" | grep -q "It can't be requested" || fail "an agent's refusal must end in the permission line: $out"

# ── show: pause words, notice, 24 h counts ──────────────────────────────
out="$("$K2" hostmail forward show sales@brand.test)"
printf '%s' "$out" | grep -q 'state=paused  reason=refused' || fail "show must print state + reason: $out"
printf '%s' "$out" | grep -q 'paused because the destination refused too many forwards' || fail "show must print pauseWords: $out"
printf '%s' "$out" | grep -q 'last 24h: forwarded 12, delivered 9, refused 3' || fail "show must print the 24 h counts: $out"
printf '%s' "$out" | grep -q 'paused' || fail "show must list events: $out"
out="$("$K2" hostmail forward show old@brand.test)"
printf '%s' "$out" | grep -q "not on: it was waiting for a confirm" || fail "an old forward must say it's off: $out"
out="$("$K2" hostmail forward show cant@brand.test)"
printf '%s' "$out" | grep -q "can't arrive: this server hosts its domain" || fail "show must say the mail can't arrive: $out"
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
printf '%s' "$out" | grep -q -- '--restart-stopped' || fail "destination_stopped must name the override: $out"

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
printf '%s' "$out" | grep -q "K2 emailed me@gmail.test to say so" || fail "person set must print the notice line: $out"
"$K2" mail forward add --to b@gmail.test >/dev/null
last_body | grep -q '^POST /cli/mail/self/forward/add ' || fail "mail forward add route: $(last_body)"
"$K2" mail forward rm --to b@gmail.test >/dev/null
last_body | grep -q '^POST /cli/mail/self/forward/rm ' || fail "mail forward rm route: $(last_body)"
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
