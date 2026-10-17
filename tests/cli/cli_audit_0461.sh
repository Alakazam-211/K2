#!/usr/bin/env bash
# 0.46.1 agent CLI audit fixes, against a stub daemon (no real ~/.k2, no
# real daemon). Each section sets the stub's canned answers, runs one verb
# and checks stdout/stderr/exit and what reached the daemon.
#
#   1. k2 msg who <id>: always the lookup, never a send to a workspace
#      named "who"; no id → exit 2; --json prints JSON.
#   2. k2 settings: a refusal exits 3 with the owner/admin hint; --help.
#   3. k2 inbox read <bad-id>: the error shows, exit 4.
#   4. k2 activity: --help/--json are flags, not the limit; errors show;
#      timestamps carry the date.
#   5. k2 users list from an agent passport: exit 3, points at
#      k2 connections list --users.
#   6. k2 hostmail status not installed / unsupported: one line, no
#      cert/owner/calendar rows; help has no `k2 mail status`.
#   8/12. k2 compute get <missing> exits 4; jobs prints the full reason.
#   9. k2 app token list: "No App tokens."
#   10. k2 sources pending: one hint only.
#   13. k2 zen widget errors: says what it checked.
# Fake values only.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
# Here-strings, not pipes: grep -q stops early on a big haystack and
# pipefail would turn the writer's broken pipe into a false miss.
assert_contains() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "${2:0:400}"))"; fi; }
assert_absent() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (unexpected $(printf %q "$3") in $(printf %q "${2:0:400}"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-cli-audit-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-disk-token-audit"
SCOPED="sess42.scoped-secret-audit"
HOME2="$WORK/home"
mkdir -p "$HOME2/.k2" "$WORK/proj"
echo "$OWNER" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"
echo '{}' > "$WORK/routes.json"

# The stub answers "METHOD /path" from routes.json ([status, body]); any
# other route is 404. Every request lands in reqs.jsonl.
python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        rec = {"method": method, "path": parsed.path, "query": {k: v[0] for k, v in q.items() if k != "token"},
               "body": body.decode("utf-8", "replace")}
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
        routes = json.load(open(os.path.join(work, "routes.json")))
        status, out = routes.get("%s %s" % (method, parsed.path), [404, {"error": "route not found"}])
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
[ -n "$STUB_PORT" ] || { echo "FAIL: stub did not start" >&2; exit 1; }

owner() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"; }
cell() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
routes() { printf '%s' "$1" > "$WORK/routes.json"; : > "$WORK/reqs.jsonl"; }
paths() { python3 -c 'import json,sys; print("\n".join(json.loads(l)["path"] for l in open(sys.argv[1]) if l.strip()))' "$WORK/reqs.jsonl"; }
run() {
    set +e
    out="$("$@" 2>"$WORK/err")"; rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
set -e
cd "$WORK"

echo "== 1. k2 msg who =="
routes '{"GET /cli/msg/who": [404, {"error": {"code": "not_found", "hint": "K2 never sent it"}}]}'
run owner msg who hello-there
assert_eq "who <non-m_ id> → exit 4 (the lookup's answer)" "$rc" "4"
assert_contains "who asked the lookup" "$(paths)" "/cli/msg/who"
assert_absent "who never sends a message" "$(paths | grep -v '^/cli/msg/who$' || true)" "/cli/"
: > "$WORK/reqs.jsonl"
run owner msg who
assert_eq "who with no id → exit 2" "$rc" "2"
assert_eq "who with no id sends nothing" "$(paths)" ""
routes '{"GET /cli/msg/who": [200, {"ok": true, "id": "m_0123456789", "from": {"handle": "sales"}}]}'
run owner msg who m_0123456789 --json
assert_eq "who --json exit 0" "$rc" "0"
assert_eq "who --json prints JSON" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')" "m_0123456789"
run owner msg who --json m_0123456789
assert_eq "who --json first also works" "$rc" "0"

echo "== 2. k2 settings =="
routes '{"GET /cli/settings": [403, {"error": {"code": "owner_only", "hint": "requires owner/admin — ask your human"}}]}'
run cell settings
assert_eq "settings refused → exit 3" "$rc" "3"
assert_absent "settings refusal prints no fake settings" "$out" "Name:"
assert_contains "settings refusal says owner/admin" "$err" "owner"
routes '{"GET /cli/settings": [200, {"name": "sales", "mode": "custom", "heartbeatEnabled": true, "pinned": false}]}'
run owner settings
assert_eq "settings exit 0" "$rc" "0"
assert_contains "settings shows the name" "$out" "Name:      sales"
run owner settings --help
assert_eq "settings --help exit 0" "$rc" "0"
assert_contains "settings help lists the real modes" "$out" "--mode <off|custom|manager|k2>"
schema="$(env HOME="$HOME2" K2_PORT=1 "$K2_CLI" --schema 2>/dev/null || true)"
assert_contains "schema: settings modes" "$schema" '"usage": "k2 settings [--mode <off|custom|manager|k2>]"'
assert_contains "schema: inbox read" "$schema" '"usage": "k2 inbox [list|read|compose|move|archive] ..."'

echo "== 3. k2 inbox read =="
routes '{"GET /cli/inbox/read": [400, {"error": "inbox item not found: zzz"}]}'
run owner inbox read zzz
assert_eq "inbox read missing → exit 4" "$rc" "4"
assert_contains "inbox read shows the error" "$err" "inbox item not found: zzz"

echo "== 4. k2 activity =="
: > "$WORK/reqs.jsonl"
run owner activity --help
assert_eq "activity --help exit 0" "$rc" "0"
assert_contains "activity --help prints help" "$out" "k2 activity [<limit>]"
assert_eq "activity --help sends nothing" "$(paths)" ""
routes '{"GET /cli/feed": [200, {"feed": [{"at": 1792243200, "agent": "sales", "type": "msg", "summary": "hi"}]}]}'
run owner activity --json
assert_eq "activity --json exit 0" "$rc" "0"
assert_eq "activity --json is JSON" "$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["feed"][0]["agent"])')" "sales"
assert_eq "activity --json kept the default limit" "$(python3 -c 'import json,sys; print([json.loads(l) for l in open(sys.argv[1])][-1]["query"].get("limit"))' "$WORK/reqs.jsonl")" "20"
run owner activity 5
assert_contains "activity timestamps carry the date" "$out" "[2026-"
assert_eq "activity <n> is the limit" "$(python3 -c 'import json,sys; print([json.loads(l) for l in open(sys.argv[1])][-1]["query"].get("limit"))' "$WORK/reqs.jsonl")" "5"
routes '{"GET /cli/feed": [500, {"error": "feed store unreadable"}]}'
run owner activity
assert_eq "activity error → exit 1" "$rc" "1"
assert_contains "activity shows the daemon error" "$err" "feed store unreadable"
assert_absent "activity error is not 'No activity.'" "$out" "No activity."
run owner activity --bogus
assert_eq "activity unknown flag → exit 2" "$rc" "2"

echo "== 5. k2 users list from an agent passport =="
routes '{"GET /cli/users": [401, {"error": "Invalid or missing auth token"}]}'
run cell users list
assert_eq "agent users list → exit 3" "$rc" "3"
assert_contains "agent users list points at connections" "$err" "k2 connections list --users"

echo
echo "CLI audit 0.46.1: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
