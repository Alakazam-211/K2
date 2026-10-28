#!/usr/bin/env bash
# k2 hostchat — the S32 verbs (K26/K27/K31) against a stub daemon.
#
# Covers:
#   1. help names app-url, ship, mirror, app-space(s) and open; the schema
#      line lists them.
#   2. Request shapes: app-url set/clear (POST /cli/sew/server/app-url),
#      app-url show reads status; ship/mirror dir <relative> sends an
#      absolute path, none sends {kind:none}; app-space bind/unbind send
#      {app, spaceId}; app-spaces lists (GET /cli/sew/app-spaces).
#   3. Exit codes: usage errors → 2 before any request; a 400 → 2; a 403 → 3.
#   4. open is person-only: inside a K2 terminal it refuses before any
#      request; with no app URL it stops before any vault confirm or mint.
#   5. status prints the app URL, off-box copy, key mirror and owner state.
# Fake values only. Never touches the real ~/.k2.

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

WORK="$(mktemp -d -t k2-hostchat-s32-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

echo "== help + schema (no daemon) =="
HOME1="$WORK/home1"
mkdir -p "$HOME1"
set +e
out="$(env HOME="$HOME1" K2_PORT=1 "$K2_CLI" hostchat --help 2>&1)"; rc=$?
set -e
assert_eq "help exit 0" "$rc" "0"
assert_contains "help: app-url" "$out" "k2 hostchat app-url set https://HOST[/path] | clear | show"
assert_contains "help: ship" "$out" "k2 hostchat ship dir <path> | none"
assert_contains "help: mirror" "$out" "k2 hostchat mirror dir <path> | none"
assert_contains "help: never the vault" "$out" "Never the K2 vault"
assert_contains "help: app spaces" "$out" "k2 hostchat app-space bind <app> <space-id>"
assert_contains "help: app grants only" "$out" "members: the App's users who also have chat; never Connect owners/admins by role"
assert_contains "help: open" "$out" "k2 hostchat open [--print]"
schema="$(env HOME="$HOME1" "$K2_CLI" --schema)"
python3 -c '
import json, sys
cmds = {c["name"]: c for c in json.loads(sys.argv[1])["commands"]}
u = cmds["hostchat"]["usage"]
for verb in ("app-url", "ship", "mirror", "app-space", "open"):
    assert verb in u, verb
for n in ("hostchat app-url", "hostchat ship", "hostchat mirror", "hostchat app-space", "hostchat open"):
    assert n in cmds, n
' "$schema" && ok "schema lists the S32 verbs" || bad "schema lists the S32 verbs"

echo "== stub daemon =="
OWNER="owner-disk-token-s32"
SCOPED="sess42.scoped-secret-s32"
HOME2="$WORK/home2"
mkdir -p "$HOME2/.k2"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
state = {"appUrl": None}

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        rec = {"method": method, "path": parsed.path, "token": (q.get("token") or [""])[0], "body": body.decode()}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        b = json.loads(body) if body else {}
        p = parsed.path
        status, out = 200, {"ok": True}
        if p == "/cli/sew/status":
            out = {"ok": True, "supported": True, "state": "running", "installId": "48831765147549701",
                   "appUrl": state["appUrl"], "owner": "set", "alarms": [], "warnings": [],
                   "backup": {"ship": {"kind": "dir", "path": "/mnt/offbox/sew", "lastShippedId": "20260103T000000Z", "lastError": None}},
                   "mirror": {"kind": "dir", "path": "/mnt/keys"}}
        elif p == "/cli/sew/server/app-url":
            u = b.get("url")
            if u and not u.startswith("https://"):
                status, out = 400, {"ok": False, "error": {"code": "invalid_url", "hint": "the app URL is https://<host>"}}
            else:
                state["appUrl"] = u or None
                out = {"ok": True, "appUrl": state["appUrl"]}
        elif p in ("/cli/sew/server/ship", "/cli/sew/server/mirror"):
            if b.get("path", "").startswith("/forbidden"):
                status, out = 403, {"ok": False, "error": {"code": "chat_manage_off", "hint": "Manage Sew is off"}}
            else:
                out = {"ok": True, "echo": b}
        elif p == "/cli/sew/app-spaces":
            out = {"ok": True, "spaces": [{"appId": "hearth", "spaceId": "spc_1", "boundAt": 1, "boundBy": "owner",
                                           "members": {"added": 3, "held": 1, "pending": 0}}],
                   "unbound": [{"appId": "oldapp", "spaceId": "spc_9", "wantedAt": 1, "wantedBy": "system:app-removed",
                                "doneAt": 2, "orphanedFrom": "oldapp"},
                               {"appId": "gone2", "spaceId": "spc_8", "wantedAt": 1, "wantedBy": "system:app-removed",
                                "doneAt": None, "orphanedFrom": None}],
                   "note": "members come only from each App's user grants (k2 app grant); Admin Access never adds anyone"}
        elif p.startswith("/cli/sew/app-spaces/"):
            out = {"ok": True, "app": b.get("app"), "spaceId": b.get("spaceId")}
        elif p == "/cli/sew/owner-handoff":
            out = {"ok": True, "code": "k2sho_" + "A" * 43, "expiresInMs": 60000}
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
cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
nreq() { if [ -f "$WORK/reqs.jsonl" ]; then grep -c '"path": "/cli/sew/' "$WORK/reqs.jsonl" || true; else echo 0; fi; }
req_where() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
rows = [r for r in rows if r["path"] == sys.argv[2]]
r = rows[-1]
print(r[sys.argv[3]] if sys.argv[3] != "body" else json.dumps(json.loads(r["body"] or "{}"), sort_keys=True))
' "$WORK/reqs.jsonl" "$1" "$2"
}
run() {
    set +e
    out="$("$@" 2>"$WORK/err")"; rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
mkdir -p "$WORK/proj" "$WORK/offbox"
cd "$WORK"

echo "== app-url =="
run owner hostchat app-url show
assert_eq "show exit 0" "$rc" "0"
assert_eq "show: not set" "$out" "(not set)"
run owner hostchat app-url set https://hearth.box.example.test/
assert_eq "set exit 0" "$rc" "0"
assert_eq "set is a POST with the owner token" "$(req_where /cli/sew/server/app-url method)/$(req_where /cli/sew/server/app-url token)" "POST/$OWNER"
assert_eq "set body" "$(req_where /cli/sew/server/app-url body)" '{"url": "https://hearth.box.example.test/"}'
run owner hostchat app-url show --json
assert_eq "show --json" "$out" '{"appUrl": "https://hearth.box.example.test/"}'
run owner hostchat app-url set http://hearth.example.test
assert_eq "invalid url → exit 2" "$rc" "2"
assert_contains "invalid url says why" "$err" "invalid_url"
run owner hostchat app-url clear
assert_eq "clear body" "$(req_where /cli/sew/server/app-url body)" '{"url": ""}'
before="$(nreq)"
run owner hostchat app-url set
assert_eq "set without a url → exit 2" "$rc" "2"
run owner hostchat app-url frob
assert_eq "unknown app-url verb → exit 2" "$rc" "2"
assert_eq "usage errors send nothing" "$(nreq)" "$before"

echo "== ship / mirror =="
run owner hostchat ship dir offbox
assert_eq "ship dir exit 0" "$rc" "0"
assert_eq "ship dir body (absolute)" "$(req_where /cli/sew/server/ship body)" "{\"kind\": \"dir\", \"path\": \"$WORK/offbox\"}"
run owner hostchat ship none
assert_eq "ship none body" "$(req_where /cli/sew/server/ship body)" '{"kind": "none"}'
run owner hostchat mirror dir /mnt/keys
assert_eq "mirror dir body" "$(req_where /cli/sew/server/mirror body)" '{"kind": "dir", "path": "/mnt/keys"}'
run owner hostchat mirror none --json
assert_eq "mirror none --json is raw" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["echo"]["kind"])')" "none"
run owner hostchat ship dir /forbidden/x
assert_eq "403 → exit 3" "$rc" "3"
before="$(nreq)"
run owner hostchat ship dir
assert_eq "ship dir without a path → exit 2" "$rc" "2"
run owner hostchat mirror s3
assert_eq "mirror s3 → exit 2" "$rc" "2"
assert_eq "ship/mirror usage sends nothing" "$(nreq)" "$before"

echo "== app spaces =="
run owner hostchat app-spaces
assert_eq "list exit 0" "$rc" "0"
assert_eq "list is a GET" "$(req_where /cli/sew/app-spaces method)" "GET"
assert_contains "list row" "$out" "hearth"
assert_contains "list counts" "$out" "added 3, held 1, pending 0"
assert_contains "unbound Spaces (Sew 0.5.0)" "$out" "Unbound Spaces (an App removed, or app-space unbind; Sew keeps each one read-only"
assert_contains "how to reuse one" "$out" "the Sew install owner releases it in Sew first"
assert_contains "an unbound Space" "$out" "spc_9"
assert_contains "unbound in Sew" "$out" "read-only in Sew"
assert_contains "not sent yet" "$out" "waiting: sent once Sew can unbind"
assert_contains "list note (stderr)" "$err" "Admin Access never adds anyone"
run owner hostchat app-space bind hearth spc_1
assert_eq "bind exit 0" "$rc" "0"
assert_eq "bind body" "$(req_where /cli/sew/app-spaces/bind body)" '{"app": "hearth", "spaceId": "spc_1"}'
run owner hostchat app-space unbind hearth spc_1 --json
assert_eq "unbind body (default: members stay)" "$(req_where /cli/sew/app-spaces/unbind body)" '{"app": "hearth", "spaceId": "spc_1"}'
run owner hostchat app-space unbind hearth spc_1 --remove-app-members --json
assert_eq "unbind --remove-app-members body" "$(req_where /cli/sew/app-spaces/unbind body)" '{"app": "hearth", "removeAppMembers": true, "spaceId": "spc_1"}'
before="$(nreq)"
run owner hostchat app-space bind hearth spc_1 --remove-app-members
assert_eq "--remove-app-members on bind → exit 2, nothing sent" "$rc/$(nreq)" "2/$before"
before="$(nreq)"
run owner hostchat app-space bind hearth
assert_eq "bind without a space → exit 2" "$rc" "2"
assert_eq "bind usage sends nothing" "$(nreq)" "$before"

echo "== open (person only) =="
before="$(nreq)"
run cell hostchat open --print
assert_eq "open inside a K2 terminal → exit 3" "$rc" "3"
assert_contains "open says a person does it" "$err" "person_only"
assert_eq "the refusal sends nothing" "$(nreq)" "$before"
owner hostchat app-url clear >/dev/null
run owner hostchat open --print
assert_eq "open with no app URL → exit 3" "$rc" "3"
assert_contains "open names the fix" "$err" "k2 hostchat app-url set"
if grep -q '"/cli/sew/owner-handoff"' "$WORK/reqs.jsonl"; then bad "no mint without an app URL"; else ok "no mint without an app URL"; fi

echo "== status lines =="
owner hostchat app-url set https://hearth.box.example.test >/dev/null
run owner hostchat status
assert_contains "status: app URL" "$out" "https://hearth.box.example.test"
assert_contains "status: off-box copy" "$out" "/mnt/offbox/sew"
assert_contains "status: last copied" "$out" "20260103T000000Z"
assert_contains "status: key mirror" "$out" "/mnt/keys"
assert_contains "status: owner in Sew" "$out" "owner in Sew:"

echo
echo "hostchat S32 CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
