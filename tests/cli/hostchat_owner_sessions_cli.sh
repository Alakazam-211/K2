#!/usr/bin/env bash
# k2 hostchat owner-sessions: list and end the owner chat sessions K2
# opened, against a stub daemon and a stub vault.sock.
#
# Covers:
#   1. Help without a daemon: the verb's own help and the hostchat help say
#      when an end takes effect, that --all also ends linked Connect
#      sessions, and that end --all ends an unlisted session.
#   2. Help never acts: -h / --help anywhere on an end line prints help and
#      makes no request.
#   3. list: GET /cli/sew/owner-sessions with the owner token as a Bearer
#      header; the rows; --json is the daemon's JSON.
#   4. end <own_…> and end --all: send, get 403 confirm_required, confirm
#      through vault.sock bound to the sid or "all", resend with
#      confirmToken; the answer says what was queued and when it acts.
#   5. A K2 terminal (agent passport) never ends one: exit 3 person_only,
#      no request. Bad usage: exit 2, no request.
# Fake values only. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

# Short path: the stub vault.sock must fit a Unix socket path.
WORK="$(mktemp -d /tmp/k2os.XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

SID="own_0123456789abcdef0123456789abcdef"

run() {
    set +e
    out="$("$@" 2>"$WORK/err")"; rc=$?
    err="$(cat "$WORK/err")"
    set -e
}

# ── 1. help, no daemon ───────────────────────────────────────────────
echo "== help (no daemon) =="
HOME1="$WORK/h1"
mkdir -p "$HOME1"
nodaemon() { env HOME="$HOME1" K2_PORT=1 "$K2_CLI" "$@" </dev/null; }
run nodaemon hostchat owner-sessions --help
assert_eq "owner-sessions --help exit 0" "$rc" "0"
assert_contains "help: when it acts" "$out" "Ending takes effect as soon as Sew receives it, normally within seconds."
assert_contains "help: the retry timing" "$out" "retried (after 2 s, doubling up to every 5 minutes) until Sew takes it"
assert_contains "help: --all ends linked Connect sessions" "$out" "ends the owner's linked Connect sessions"
assert_contains "help: an unlisted session" "$out" "If a session isn't listed, \`end --all\` still ends it."
assert_contains "help: a person confirms" "$out" "terminal outside K2, never an agent's"
run nodaemon hostchat --help
assert_contains "hostchat help names the verb" "$out" "k2 hostchat owner-sessions [list] | owner-sessions end <own_…> | owner-sessions end --all"
assert_contains "hostchat help: when it acts" "$out" "Ending takes effect as soon as Sew receives it"
run nodaemon --schema
assert_contains "schema lists hostchat owner-sessions" "$out" '"name": "hostchat owner-sessions"'

# ── stub daemon + vault.sock ─────────────────────────────────────────
OWNER="owner-disk-token-ownsess"
SCOPED="sess88.scoped-secret-ownsess"
HOME2="$WORK/h2"
mkdir -p "$HOME2/.k2"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" "$SCOPED" "$HOME2/.k2/vault.sock" "$SID" <<'PYEOF' &
import json, os, socketserver, sys, threading, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work, scoped, vsock, sid = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
ACTION = "chat-owner-sessions-end"
EFFECT = "Ending takes effect as soon as Sew receives it, normally within seconds."

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = {k: v[0] for k, v in urllib.parse.parse_qs(parsed.query).items()}
        auth = self.headers.get("Authorization") or ""
        token = auth[len("Bearer "):] if auth.startswith("Bearer ") else ""
        rec = {"method": method, "path": parsed.path, "token": token, "query": q, "body": body.decode("utf-8", "replace")}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        b = json.loads(body) if body else {}
        p = parsed.path
        status, out = 200, {"ok": True}
        def err(st, code, hint, details=None):
            e = {"code": code, "hint": hint}
            if details:
                e["details"] = details
            return st, {"ok": False, "error": e}
        if p == "/cli/vault/confirm/start":
            out = {"methods": ["vault-password"]}
        elif p == "/cli/vault/confirm/finish":
            out = {"token": "tok-" + str(b.get("action")) + "-" + str(b.get("secretId"))}
        elif token == scoped and p.startswith("/cli/sew/owner-sessions"):
            status, out = err(403, "person_only", "the owner's chat sessions are for a person to see and end, never an agent")
        elif p == "/cli/sew/owner-sessions" and method == "GET":
            out = {"ok": True, "takesEffect": EFFECT, "sessions": [
                {"sid": sid, "mintedBy": "owner-token", "mintedAt": 1760000000, "redeemedAt": 1760000010,
                 "endQueued": False, "delivered": False},
                {"sid": "own_" + "b" * 32, "mintedBy": "user:ada", "mintedAt": 1760000000, "redeemedAt": 1760000010,
                 "endQueued": True, "delivered": False,
                 "end": {"queuedAt": 1760000100, "via": "all", "by": "login:ada", "outbox": "pending"}},
                {"sid": "own_" + "c" * 32, "mintedBy": "owner-token", "mintedAt": 1760000000, "redeemedAt": 1760000010,
                 "endQueued": True, "delivered": True,
                 "end": {"queuedAt": 1760000100, "via": "sid", "by": "owner", "outbox": "done"}}]}
        elif p == "/cli/sew/owner-sessions/revoke" and method == "POST":
            secret = "all" if b.get("all") else b.get("sid")
            if not b.get("confirmToken"):
                status, out = err(403, "confirm_required", "this needs a fresh vault confirm from a person",
                                  {"action": ACTION, "secretId": secret})
            elif b["confirmToken"] != "tok-%s-%s" % (ACTION, secret):
                status, out = err(403, "confirm_required", "wrong confirm")
            else:
                out = {"ok": True, "queued": "external_id=k2:owner" if b.get("all") else "k2_sid=" + secret,
                       "ended": [sid] if not b.get("all") else [sid, "own_" + "d" * 32],
                       "confirmedBy": "vault-password", "takesEffect": EFFECT}
        else:
            status, out = 404, {"error": "route not found"}
        data = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

class UH(H):
    def address_string(self):
        return "unix"

if os.path.exists(vsock):
    os.unlink(vsock)
us = U(vsock, UH)
threading.Thread(target=us.serve_forever, daemon=True).start()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && [ -S "$HOME2/.k2/vault.sock" ] && break; sleep 0.05; done
STUB_PORT="$(cat "$WORK/stub.port")"
touch "$WORK/reqs.jsonl"

owner() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" "$@" </dev/null; }
cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@" </dev/null
}
# Requests to the routes under test and the vault (a K2 terminal's CLI may
# make other calls first, such as resolving its session).
nreq() { grep -cE '"path": "/cli/(sew|vault)/' "$WORK/reqs.jsonl" || true; }
req_where() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
rows = [r for r in rows if r["path"] == sys.argv[2]]
r = rows[int(sys.argv[4]) if len(sys.argv) > 4 else -1]
f = sys.argv[3]
if f == "body":
    print(json.dumps(json.loads(r["body"] or "{}"), sort_keys=True))
else:
    print(r[f])
' "$WORK/reqs.jsonl" "$@"
}
count_path() { python3 -c '
import json, sys
print(sum(1 for l in open(sys.argv[1]) if json.loads(l)["path"] == sys.argv[2]))' "$WORK/reqs.jsonl" "$1"; }
mkdir -p "$WORK/proj"
cd "$WORK"

# ── 2. help never acts ───────────────────────────────────────────────
echo "== help never acts =="
for line in "owner-sessions end $SID -h" "owner-sessions end $SID --help" "owner-sessions end --all -h" \
            "owner-sessions end -h --all" "owner-sessions -h end --all" "owner-sessions revoke --all --help"; do
    before="$(nreq)"
    # shellcheck disable=SC2086
    run owner hostchat $line
    assert_eq "help: hostchat $line → exit 0, no request" "$rc/$(nreq)" "0/$before"
    assert_contains "help: hostchat $line prints the verb's help" "$out" "k2 hostchat owner-sessions end --all"
done

# ── 3. list ──────────────────────────────────────────────────────────
echo "== list =="
run owner hostchat owner-sessions
assert_eq "list exit 0" "$rc" "0"
assert_eq "list: GET with the owner token" "$(req_where /cli/sew/owner-sessions method)/$(req_where /cli/sew/owner-sessions token)" "GET/$OWNER"
assert_contains "list: a live session" "$out" "$SID  minted"
assert_contains "list: who opened it" "$out" "by owner-token"
assert_contains "list: still live" "$out" "live until Sew ends it"
assert_contains "list: an end queued by --all" "$out" "by login:ada (all), outbox pending"
assert_contains "list: an end Sew took" "$out" "ended (Sew took the end"
run owner hostchat owner-sessions list --json
assert_eq "list --json is the daemon's JSON" "$(printf '%s' "$out" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(len(v["sessions"]), v["sessions"][0]["sid"])')" "3 $SID"

# ── 4. end with a person confirm ─────────────────────────────────────
echo "== end one =="
run owner hostchat owner-sessions end "$SID"
assert_eq "end <sid> exit 0" "$rc" "0"
assert_eq "end: two POSTs" "$(count_path /cli/sew/owner-sessions/revoke)" "2"
assert_eq "end: first without a token" "$(req_where /cli/sew/owner-sessions/revoke body 0)" "{\"sid\": \"$SID\"}"
assert_eq "end: confirm bound to the sid" "$(req_where /cli/vault/confirm/finish body)" \
    "{\"action\": \"chat-owner-sessions-end\", \"method\": \"vault-password\", \"password\": \"\", \"secretId\": \"$SID\"}"
assert_eq "end: resent with the token" "$(req_where /cli/sew/owner-sessions/revoke body)" \
    "{\"confirmToken\": \"tok-chat-owner-sessions-end-$SID\", \"sid\": \"$SID\"}"
assert_contains "end: what was queued" "$out" "end queued: k2_sid=$SID"
assert_contains "end: when it acts" "$out" "Ending takes effect as soon as Sew receives it"

echo "== end --all =="
run owner hostchat owner-sessions end --all
assert_eq "end --all exit 0" "$rc" "0"
assert_eq "end --all: first body" "$(req_where /cli/sew/owner-sessions/revoke body 2)" '{"all": true}'
assert_eq "end --all: confirm bound to all" "$(req_where /cli/vault/confirm/finish body)" \
    '{"action": "chat-owner-sessions-end", "method": "vault-password", "password": "", "secretId": "all"}'
assert_eq "end --all: resent with the token" "$(req_where /cli/sew/owner-sessions/revoke body)" \
    '{"all": true, "confirmToken": "tok-chat-owner-sessions-end-all"}'
assert_contains "end --all: k2:owner" "$out" "end queued: external_id=k2:owner"
assert_contains "end --all: marked" "$out" "marked: $SID, own_"
run owner hostchat owner-sessions revoke --all --json
assert_eq "revoke = end; --json raw" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["queued"])')" "external_id=k2:owner"

# ── 5. agents and bad usage ──────────────────────────────────────────
echo "== refused and usage =="
fin="$(count_path /cli/vault/confirm/finish)"
before="$(nreq)"
run cell hostchat owner-sessions end "$SID"
assert_eq "agent end → exit 3, nothing sent" "$rc/$(nreq)" "3/$before"
assert_contains "agent end: person only" "$err" "person_only"
run cell hostchat owner-sessions end --all
assert_eq "agent end --all → exit 3, nothing sent" "$rc/$(nreq)" "3/$before"
assert_eq "agent: no confirm attempted" "$(count_path /cli/vault/confirm/finish)" "$fin"
run cell hostchat owner-sessions
assert_eq "agent list → the daemon refuses (exit 3)" "$rc" "3"
assert_eq "agent list: sent with the passport" "$(req_where /cli/sew/owner-sessions token)" "$SCOPED"
assert_contains "agent list: person only" "$err" "person_only"
before="$(nreq)"
for line in "owner-sessions end" "owner-sessions end $SID --all" "owner-sessions end not-a-sid" \
            "owner-sessions end $SID own_other" "owner-sessions list --bogus" "owner-sessions list $SID" "owner-sessions forget"; do
    # shellcheck disable=SC2086
    run owner hostchat $line
    assert_eq "usage: hostchat $line → exit 2, nothing sent" "$rc/$(nreq)" "2/$before"
done

# No request carries a credential in its query string.
in_query="$(python3 -c '
import json, sys
print(sum(1 for l in open(sys.argv[1]) if "token" in json.loads(l)["query"]))' "$WORK/reqs.jsonl")"
assert_eq "no request has token= in its query" "$in_query" "0"

echo
echo "hostchat owner-sessions CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
