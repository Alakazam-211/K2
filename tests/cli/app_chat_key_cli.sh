#!/usr/bin/env bash
# App-governed Spaces S1/S6 (0.46.7): `k2 app grant <app> --kind chat-key`
# and `k2 hostchat pass-keys`, against a stub daemon.
#
# Covers: the request shapes (POST /cli/app/chat-key with only the flags
# given, /cli/app/chat-key/revoke, GET /cli/sew/keys, POST
# /cli/sew/keys/{rotate,drop-previous}); usage errors exit 2 before any
# request; the human output (a revoked key shows "(off)" in `k2 app
# grants`); a refusal exits 3. Hermetic: HOME is a throwaway dir, every
# K2_*/K2SO_* var is unset, the daemon is a tiny python stub on loopback.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

WORK="$(mktemp -d -t k2-chat-key-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

HOME2="$WORK/home"
mkdir -p "$HOME2/.k2"
echo "owner-disk-token-chat-key" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
APP = {"serviceId": "0f8fad5b-d9cb-469f-a165-70867728950e", "name": "jobs", "handle": "crew/jobs", "appId": "jobs"}
KEY = {"kind": "chat-key", "enabled": True, "caps": ["chat:read", "voice:join"], "maxLevel": "moderator", "uploads": False,
       "maxMembers": 200, "grantedBy": "owner", "grantedAt": 1790510400, "revokedAt": None}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        n = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(n) if n else b""
        parsed = urllib.parse.urlparse(self.path)
        rec = {"method": method, "path": parsed.path, "body": body.decode("utf-8", "replace")}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        p = parsed.path
        status, out = 200, {"ok": True}
        if p == "/cli/app/chat-key" and os.path.exists(os.path.join(work, "refuse")):
            status, out = 403, {"ok": False, "error": {"code": "owner_only", "hint": "only the server's owner, in person, turns an App's chat key on"}}
        elif p == "/cli/app/chat-key":
            out = {"ok": True, "app": APP, "key": KEY, "before": None, "widened": True}
        elif p == "/cli/app/chat-key/revoke":
            off = dict(KEY, enabled=False, revokedAt=1790510500)
            out = {"ok": True, "app": APP, "revoked": True, "sessionsEnded": True, "key": off}
        elif p == "/cli/app/grants":
            out = {"ok": True, "app": APP, "grants": [dict(KEY, enabled=False)]}
        elif p == "/cli/sew/keys":
            out = {"ok": True, "exists": True, "keys": [
                {"kid": "k2cp-aaaa", "state": "current", "createdAt": 1790510000, "publishedAt": 1790510000, "rotateDueAt": 1793102000},
                {"kid": "k2cp-bbbb", "state": "next", "createdAt": 1790510100, "publishedAt": 1790510100}]}
        elif p in ("/cli/sew/keys/rotate", "/cli/sew/keys/drop-previous"):
            out = {"ok": True, "exists": True, "keys": [{"kid": "k2cp-bbbb", "state": "current", "publishedAt": 1790510100}]}
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

owner() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" "$@"; }
nreq() { if [ -f "$WORK/reqs.jsonl" ]; then wc -l < "$WORK/reqs.jsonl" | tr -d ' '; else echo 0; fi; }
# The last request to PATH: "METHOD BODY".
last() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if json.loads(l)["path"] == sys.argv[2]]
r = rows[-1]
print(r["method"], json.dumps(json.loads(r["body"]), sort_keys=True) if r["body"] else "-")' "$WORK/reqs.jsonl" "$1"
}

echo "== k2 app grant --kind chat-key =="
out="$(owner app grant jobs --kind chat-key --caps "chat:read, voice:join" --max-level moderator --uploads off --max 200)"
assert_eq "on: the request" "$(last /cli/app/chat-key)" \
    'POST {"app": "jobs", "caps": ["chat:read", "voice:join"], "maxLevel": "moderator", "maxMembers": 200, "uploads": false}'
assert_contains "on: says what it is" "$out" "jobs holds a chat key"
assert_contains "on: the ceiling" "$out" "caps chat:read voice:join, up to level moderator, up to 200 people, uploads off"
owner app grant jobs --kind chat-key --max-level member >/dev/null
assert_eq "a change sends only the flags given" "$(last /cli/app/chat-key)" 'POST {"app": "jobs", "maxLevel": "member"}'
out="$(owner app grant jobs --kind chat-key --revoke)"
assert_eq "revoke: the request" "$(last /cli/app/chat-key/revoke)" 'POST {"app": "jobs"}'
assert_contains "revoke: off" "$out" "chat key off (no more passes through it)"
assert_contains "revoke: sessions" "$out" "Sew ends this App's chat sessions"
out="$(owner app grants jobs)"
assert_contains "grants: an off key shows off" "$out" "chat-key  (off) chat:read voice:join, up to level moderator"
out="$(owner app grant jobs --kind chat-key --json)"
assert_contains "--json: raw" "$out" '"widened": true'

before="$(nreq)"
set +e
err="$(owner app grant jobs --kind chat-key --uploads maybe 2>&1)"; rc=$?
set -e
assert_eq "--uploads maybe: exit 2" "$rc" "2"
assert_contains "--uploads maybe: says so" "$err" "--uploads on|off"
assert_eq "--uploads maybe: no request" "$(nreq)" "$before"

touch "$WORK/refuse"
set +e
err="$(owner app grant jobs --kind chat-key 2>&1)"; rc=$?
set -e
assert_eq "a refusal exits 3" "$rc" "3"
assert_contains "a refusal names its code" "$err" "owner_only"
rm -f "$WORK/refuse"

echo "== k2 hostchat pass-keys =="
out="$(owner hostchat pass-keys)"
assert_eq "show: the request" "$(last /cli/sew/keys)" "GET -"
assert_contains "show: current" "$out" "current   k2cp-aaaa"
assert_contains "show: next" "$out" "next      k2cp-bbbb"
out="$(owner hostchat pass-keys rotate)"
assert_eq "rotate: the request" "$(last /cli/sew/keys/rotate)" "POST {}"
assert_contains "rotate: says so" "$out" "rotated: the next key signs chat passes now"
owner hostchat pass-keys drop-previous --json >/dev/null
assert_eq "drop-previous: the request" "$(last /cli/sew/keys/drop-previous)" "POST {}"
before="$(nreq)"
set +e
err="$(owner hostchat pass-keys wipe 2>&1)"; rc=$?
set -e
assert_eq "an unknown step: exit 2" "$rc" "2"
assert_contains "an unknown step: usage" "$err" "k2 hostchat pass-keys [rotate|drop-previous]"
assert_eq "an unknown step: no request" "$(nreq)" "$before"
out="$(env HOME="$WORK/nohome" K2_PORT=1 "$K2_CLI" hostchat --help 2>&1)"
assert_contains "help: pass-keys" "$out" "k2 hostchat pass-keys rotate [--json]"
assert_contains "help: one set per box" "$out" "never one per App"
# k2 app chat --help prints its own page (it printed the whole k2 app
# family before 0.46.7's S8), with the hold reasons the sync can give.
before="$(nreq)"
out="$(owner app chat --help 2>&1)"
assert_eq "app chat help: no request" "$(nreq)" "$before"
assert_contains "app chat help: its own page" "$out" "What K2 keeps in step for an App's chat"
assert_contains "app chat help: the member limit hold" "$out" "the Space at the App's member limit"
if printf '%s' "$out" | grep -Fq 'k2 app token <create|list|revoke|rooms>'; then bad "app chat help: not the family's usage"; else ok "app chat help: not the family's usage"; fi

echo
echo "app chat-key CLI: $pass passed, $fail failed"
[ "$fail" = 0 ]
