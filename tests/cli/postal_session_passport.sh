#!/usr/bin/env bash
# In a session, `k2 postal` acts as the session: every request carries the
# session's passport, never the owner token from disk (0.46.17).
#
# `postal` is an Open verb, so inside a K2 terminal the CLI's Open-verb
# rule reads the owner token from disk. In a session (the K2_HOOK_SOCK
# marker) every Postal verb that reaches the daemon sends the passport as
# its Bearer header instead, and `k2 msg <postal address>` does too. The
# daemon admits a passport on every /cli/postal/* route and decides as the
# agent: a read gives the same answer, claim, invite, enable, link-p5 and
# accept put a card up (exit 2), and the owner's steps (disable,
# admins-may-accept, code, owner-link) are refused (exit 3), ending with
# where a person takes them. A session with no passport sends nothing and
# exits 3. Outside a session (the owner's own terminal) the owner token is
# sent, as before.
#
# Each session case runs twice: once with no cell socket at the marker's
# path (every request goes over TCP), once with a live one (the stub serves
# both and logs which carried each request). With a live socket the routes
# the daemon's cell socket serves (status, friends, claim, invite, decide,
# enable, link-p5, remove, block, route, send) go over it; code, settings
# and owner-link, which it doesn't serve, go over TCP with the passport.
#
# Nothing is installed: the daemon is a stub that logs each request's
# method, path, query, Bearer header and transport. The CLI runs with a
# clean environment (`env -i`, a temp HOME whose .k2 holds a readable owner
# token), so it can only reach the stub.
# Run with: bash tests/cli/postal_session_passport.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
PY="$(command -v python3)" || { echo "FAIL: python3 is required for the stub daemon" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (no $(printf %q "$3") in $(printf %q "$2"))" ;; esac; }

WORK="$(mktemp -d -t k2-postal-passport-XXXXXX)"
STUB_PID=""
# A Unix socket path must be short (about 100 bytes): its own folder.
SOCK_DIR="$(mktemp -d /tmp/k2pp-XXXXXX)"
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK" "$SOCK_DIR"
}
trap cleanup EXIT

OWNER="owner-tok-71c2e9"
PASSPORT="sess42.passport8d1b"
LOG_FILE="$WORK/requests"
SOCK="$SOCK_DIR/cell.sock"

cat >"$WORK/stub.py" <<'PY'
import json, os, socketserver, threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

LOG_FILE = os.environ["LOG_FILE"]
OWNER = os.environ["OWNER"]
PASSPORT = os.environ["PASSPORT"]
REFUSED = '{"error":"Invalid or missing auth token"}'
ADDR = "scout::acme.postal.bot"
PEER = "pal::globex.postal.bot"
# The routes the daemon's cell socket serves (session_token.rs ALLOW_EXACT).
CELL = {"/cli/postal/" + p for p in ("status", "friends", "invite", "remove", "block", "link-p5", "claim", "decide", "enable", "send", "route")}
STATUS = {"ok": True, "handle": {"address": ADDR, "status": "claimed"}, "counts": {"accepted": 1, "pendingIn": 0, "pendingOut": 0}}
SETTINGS = {"ok": True, "enabled": True, "adminsMayAccept": False}
ASKED = {"ok": True, "asked": True, "card": "c-1", "request": "pr_7", "waiting": False,
         "hint": "a person answers the card on your pinned Thread; the answer comes back to you as a [permission:] turn"}
FRIEND = {"ok": True, "friend": {"peer": PEER, "status": "accepted"}, "friends": True}
SENT = {"success": True, "status": "held", "to": PEER, "badge": "b-1", "deliveryId": "p_0123456789abcdef0123"}

def refusal(code, hint):
    return 403, {"ok": False, "error": {"code": code, "hint": hint}}

def answer(path, method, who, body):
    # who: "owner" or "agent" (a passport). As postal_routes::handle decides.
    agent = who == "agent"
    if path == "/cli/postal/status" and method == "GET":
        return 200, STATUS
    if path == "/cli/postal/friends" and method == "GET":
        return 200, {"ok": True, "handle": STATUS["handle"], "friends": [{"peer": PEER, "status": "accepted", "updatedAt": 0}]}
    if path == "/cli/postal/settings":
        if method == "GET":
            return 200, SETTINGS
        if agent:
            return refusal("owner_only", "the server's owner turns Postal on or off")
        return 200, dict(SETTINGS, enabled=bool(body.get("enabled", True)))
    if path == "/cli/postal/code":
        if agent:
            return refusal("postal_people_only", "a pairing code is for the person who answers the request (the Owner, or an Admin when the Owner allows it), never an agent")
        return 200, {"ok": True, "id": "pr_7", "peer": PEER, "code": "123 456"}
    if path == "/cli/postal/owner-link":
        if agent:
            return refusal("owner_only", "the server's owner links Postal approvals")
        return 200, {"ok": True, "label": "acme", "url": "https://approve.example.invalid/d", "userCode": "ABCD-EFGH"}
    if path in ("/cli/postal/claim", "/cli/postal/enable", "/cli/postal/link-p5"):
        if agent:
            return 200, ASKED
        if path == "/cli/postal/enable":
            return 200, SETTINGS
        return 200, STATUS
    if path == "/cli/postal/invite":
        if agent:
            return 200, ASKED
        return 200, {"ok": True, "friend": {"peer": PEER, "status": "pending_out"}, "sent": True}
    if path == "/cli/postal/decide":
        if agent and body.get("decision") == "accept":
            return 200, ASKED
        return 200, FRIEND
    if path in ("/cli/postal/remove", "/cli/postal/block"):
        return 200, {"ok": True, "friend": {"peer": PEER, "status": "revoked"}}
    if path == "/cli/postal/route":
        return 200, {"ok": True, "route": {"targets": body.get("targets", "").split(), "wake": True}}
    if path == "/cli/postal/send":
        return 200, SENT
    return 404, {"error": "route not found"}

def handler(via):
    class H(BaseHTTPRequestHandler):
        def _send(self, code, obj):
            data = (obj if isinstance(obj, str) else json.dumps(obj)).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def _answer(self, method, raw):
            u = urlsplit(self.path)
            auth = self.headers.get("Authorization")
            with open(LOG_FILE, "a") as f:
                f.write(json.dumps({"via": via, "method": method, "path": u.path, "query": u.query, "auth": auth, "body": raw.decode("utf-8", "replace")}) + "\n")
            if u.path == "/boot-status":
                return self._send(200, {"view": "full", "phase": "ready", "features": ["send-once"]})
            if auth == "Bearer " + PASSPORT:
                who = "agent"
            elif via == "tcp" and auth == "Bearer " + OWNER:
                who = "owner"
            else:
                return self._send(403, REFUSED)
            if u.path == "/cli/whoami":
                return self._send(200, {"ok": True, "workspace": "acme"})
            # The cell socket serves only its agent verbs.
            if via == "uds" and u.path not in CELL:
                return self._send(403, REFUSED)
            try:
                body = json.loads(raw) if raw else {}
            except ValueError:
                body = {}
            code, obj = answer(u.path, method, who, body if isinstance(body, dict) else {})
            return self._send(code, obj)

        def do_GET(self):
            self._answer("GET", b"")

        def do_POST(self):
            n = int(self.headers.get("Content-Length") or 0)
            self._answer("POST", self.rfile.read(n))

        def log_message(self, *_a):
            pass
    return H

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    def get_request(self):
        conn, _ = super().get_request()
        return conn, ("uds", 0)

class T(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True

u = U(os.environ["SOCK_PATH"], handler("uds"))
threading.Thread(target=u.serve_forever, daemon=True).start()
T(("127.0.0.1", int(os.environ["PORT"])), handler("tcp")).serve_forever()
PY

mkdir -p "$WORK/ws/.k2"
WS="$WORK/ws"
PORT="$("$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
: >"$LOG_FILE"
LOG_FILE="$LOG_FILE" PORT="$PORT" SOCK_PATH="$SOCK" OWNER="$OWNER" PASSPORT="$PASSPORT" "$PY" "$WORK/stub.py" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if [ -S "$SOCK" ] && curl -s --connect-timeout 1 -o /dev/null "http://127.0.0.1:${PORT}/boot-status"; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

# The owner token on disk, readable, as in a K2 terminal on the owner's box.
FAKE_HOME="$WORK/home"
mkdir -p "$FAKE_HOME/.k2"
printf '%s' "$OWNER" >"$FAKE_HOME/.k2/heartbeat.token"
chmod 600 "$FAKE_HOME/.k2/heartbeat.token"
printf '%s' "$PORT" >"$FAKE_HOME/.k2/heartbeat.port"

run() { # env words..., then --, then the k2 args
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : >"$LOG_FILE"
    set +e
    out="$(cd "$WS" && env -i PATH="/usr/bin:/bin" HOME="$FAKE_HOME" K2_HOME="$FAKE_HOME/.k2" K2_PROJECT_PATH="$WS" ${envs[@]+"${envs[@]}"} "$K2_CLI" "$@" 2>"$WORK/err" </dev/null)"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
# field <name>: that field of every logged /cli/ request, one per line.
field() {
    "$PY" -c 'import json, sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r.get("path", "").startswith("/cli/"):
        print(r.get(sys.argv[2]))' "$LOG_FILE" "$1"
}
requests() { grep -c '"path": "/cli/' "$LOG_FILE" || true; }
owner_never_sent() { # label
    if grep -Fq -- "$OWNER" "$LOG_FILE"; then
        bad "$1: the owner token was sent ($(grep -F -- "$OWNER" "$LOG_FILE" | head -n 1))"
    else
        ok "$1: the owner token is in no header, query or body"
    fi
}
no_query_credential() { # label
    case "$(field query)" in
        *token*) bad "$1: a credential in the query: $(field query)" ;;
        *) ok "$1: no credential in the query" ;;
    esac
}

# Every verb that reaches the daemon, as "label|class|args" (args split on
# spaces). class: read (exit 0, same answer), card (exit 2), done (exit 0),
# owner (a person's step: exit 3).
VERBS=(
    "status|read|postal status"
    "friends|read|postal friends"
    "settings|read|postal settings"
    "claim|card|postal claim scout::acme.postal.bot --why reach-globex"
    "invite|card|postal invite pal::globex.postal.bot --why plan-a-launch"
    "enable|card|postal enable --why reach-globex"
    "link-p5|card|postal link-p5"
    "accept|card|postal accept pr_7"
    "reject|done|postal reject pr_7"
    "remove|done|postal remove pal::globex.postal.bot"
    "block|done|postal block pal::globex.postal.bot"
    "route|done|postal route inbox"
    "postal send|done|postal send pal::globex.postal.bot hello"
    "msg to a postal address|done|msg pal::globex.postal.bot hello"
    "disable|owner|postal disable"
    "admins-may-accept|owner|postal admins-may-accept on"
    "code|owner|postal code pr_7"
    "owner-link|owner|postal owner-link"
)

# The transport a verb takes in a session with a live cell socket.
via_for() {
    case "$1" in
        settings|disable|admins-may-accept|code|owner-link) echo tcp ;;
        *) echo uds ;;
    esac
}

want_rc() {
    case "$1" in read|done) echo 0 ;; card) echo 2 ;; owner) echo 3 ;; esac
}

# 1-2. A session with a passport: no cell socket at the marker's path, then
# a live one. Every verb sends the passport and only the passport.
for SESSION in nosock cellsock; do
    if [ "$SESSION" = nosock ]; then marker="$SOCK_DIR/none.sock"; else marker="$SOCK"; fi
    echo "== session with a passport ($SESSION)"
    for v in "${VERBS[@]}"; do
        name="${v%%|*}"; rest="${v#*|}"; class="${rest%%|*}"; args="${rest#*|}"
        # shellcheck disable=SC2086
        run K2_HOOK_SOCK="$marker" K2_HOOK_TOKEN="$PASSPORT" -- $args
        label="$SESSION: $name"
        if [ "$(requests)" -ge 1 ]; then ok "$label: reached the daemon"; else bad "$label: no request (rc=$rc err=$err)"; fi
        assert_eq "$label: every request's Bearer header" "$(field auth | sort -u)" "Bearer $PASSPORT"
        if [ "$SESSION" = cellsock ]; then want_via="$(via_for "$name")"; else want_via=tcp; fi
        assert_eq "$label: every request over $want_via" "$(field via | sort -u)" "$want_via"
        no_query_credential "$label"
        owner_never_sent "$label"
        assert_eq "$label: exit code ($class)" "$rc" "$(want_rc "$class")"
        case "$class" in
            card) assert_contains "$label: says a card is up" "$out" "Asked: a card is on your person's Thread" ;;
            owner) assert_contains "$label: says where a person takes it" "$err" "Postal tab or in Settings" ;;
        esac
    done
    run K2_HOOK_SOCK="$marker" K2_HOOK_TOKEN="$PASSPORT" -- postal status
    assert_contains "$SESSION: status: the same answer as the owner's" "$out" "Address:  scout::acme.postal.bot (on the Postal network)"
    run K2_HOOK_SOCK="$marker" K2_HOOK_TOKEN="$PASSPORT" -- postal disable
    assert_contains "$SESSION: disable: the daemon's own refusal" "$err" "owner_only: the server's owner turns Postal on or off"
    run K2_HOOK_SOCK="$marker" K2_HOOK_TOKEN="$PASSPORT" -- postal code pr_7
    assert_contains "$SESSION: code: the daemon's own refusal" "$err" "postal_people_only: a pairing code is for the person"
done

# 3. A session with no passport: one line, exit 3, nothing sent.
echo "== session with no passport"
for v in "${VERBS[@]}"; do
    name="${v%%|*}"; rest="${v#*|}"; args="${rest#*|}"
    # shellcheck disable=SC2086
    run K2_HOOK_SOCK="$SOCK" -- $args
    assert_eq "no passport: $name: exit 3" "$rc" "3"
    assert_eq "no passport: $name: nothing sent" "$(requests)" "0"
    owner_never_sent "no passport: $name"
done
run K2_HOOK_SOCK="$SOCK" -- postal status
assert_contains "no passport: says nothing was sent" "$err" "this session has no passport, so nothing was sent"
assert_contains "no passport: says where a person acts" "$err" "Postal tab or in Settings"
# Help still prints in such a session, and sends nothing.
run K2_HOOK_SOCK="$SOCK" -- postal --help
assert_eq "no passport: postal --help: exit 0" "$rc" "0"
assert_eq "no passport: postal --help: nothing sent" "$(requests)" "0"

# 4. Outside a session (the owner's own terminal): the owner token, as before.
echo "== outside a session"
for v in "status|0|postal status" "claim|0|postal claim scout::acme.postal.bot --why reach-globex" \
         "disable|0|postal disable" "code|0|postal code pr_7" "postal send|0|postal send pal::globex.postal.bot hello"; do
    name="${v%%|*}"; rest="${v#*|}"; want="${rest%%|*}"; args="${rest#*|}"
    # shellcheck disable=SC2086
    run -- $args
    if [ "$(requests)" -ge 1 ]; then ok "outside: $name: reached the daemon"; else bad "outside: $name: no request (rc=$rc err=$err)"; fi
    assert_eq "outside: $name: every request's Bearer header" "$(field auth | sort -u)" "Bearer $OWNER"
    assert_eq "outside: $name: every request over TCP" "$(field via | sort -u)" "tcp"
    assert_eq "outside: $name: exit code" "$rc" "$want"
done
run -- postal code pr_7
assert_contains "outside: code: the owner reads the pairing code" "$out" "Your code: 123 456"

echo
echo "postal_session_passport: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
