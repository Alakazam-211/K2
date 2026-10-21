#!/usr/bin/env bash
# 0.46.3 leftovers, against a stub daemon (no real ~/.k2, no real daemon).
#
#   A1. k2 tunnel status: a 401 from k2.dev (the relay list) reads as a
#       notice, never "last refresh failed" (new daemon `notice`, and an
#       older daemon's `last_error: "401 …"`). Other errors still say failed.
#   A2. k2 tickets list / k2 feedback list / k2 activity: with a session
#       passport and no K2_PROJECT_PATH, the workspace is the passport's
#       (`/cli/whoami` → path), from any folder. People keep $PWD; an
#       explicit K2_PROJECT_PATH wins; no path in the answer → $PWD.
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
assert_contains() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "${2:0:400}"))"; fi; }
assert_absent() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (unexpected $(printf %q "$3") in $(printf %q "${2:0:400}"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-cli-0463-XXXXXX)"
WORK="$(cd "$WORK" && pwd -P)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER="owner-disk-token-0463"
SCOPED="sess42.scoped-secret-0463"
HOME2="$WORK/home"
mkdir -p "$HOME2/.k2" "$WORK/proj" "$WORK/elsewhere" "$WORK/agentws"
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
        q = urllib.parse.parse_qs(parsed.query, keep_blank_values=True)
        rec = {"method": method, "path": parsed.path,
               "query": {k: v[0] for k, v in q.items() if k != "token"},
               "token": (q.get("token") or [""])[0],
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
# A person in a plain terminal: no passport, no K2_PROJECT_PATH.
person() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" "$@"; }
# An agent in a K2 terminal: passport, no K2_PROJECT_PATH.
agent() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" "$K2_CLI" "$@"
}
agent_pinned() {
    env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="$SCOPED" \
        K2_HOOK_SOCK="$WORK/not-a-socket.sock" K2_PROJECT_PATH="$WORK/proj" "$K2_CLI" "$@"
}
routes() { printf '%s' "$1" > "$WORK/routes.json"; : > "$WORK/reqs.jsonl"; }
# The `project=` (or another key) of the last request to a path.
last_q() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
rows = [r for r in rows if r["path"] == sys.argv[2]]
print(rows[-1]["query"].get(sys.argv[3], "") if rows else "<no request>")
' "$WORK/reqs.jsonl" "$1" "${2:-project}"
}
last_token() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
rows = [r for r in rows if r["path"] == sys.argv[2]]
print(rows[-1]["token"] if rows else "<no request>")
' "$WORK/reqs.jsonl" "$1"
}
run() {
    set +e
    out="$("$@" 2>"$WORK/err")"; rc=$?
    err="$(cat "$WORK/err")"
    set -e
}
set -e
cd "$WORK/elsewhere"

echo "== A1. k2 tunnel status: a k2.dev 401 is not a failure =="
routes '{"GET /cli/tunnel/status": [200, {"running": true, "relay_list": {"source": "fetched", "relays": ["1.2.3.4:7000"], "refresh_s": 21600, "notice": "k2.dev does not hand out a relay list for this tunnel token; the saved list stays in use"}}]}'
run owner tunnel status
assert_eq "status exit 0" "$rc" "0"
assert_contains "notice shown" "$out" "Relay list: k2.dev does not hand out a relay list for this tunnel token"
assert_absent "no failure claimed" "$out" "failed"
routes '{"GET /cli/tunnel/status": [200, {"running": true, "relay_list": {"source": "provisioned", "relays": ["1.2.3.4:7000"], "refresh_s": 21600, "last_error": "401 unauthorized (tunnel token refused)"}}]}'
run owner tunnel status
assert_contains "older daemon 401: notice wording" "$out" "the saved list stays in use"
assert_absent "older daemon 401: no failure claimed" "$out" "failed"
routes '{"GET /cli/tunnel/status": [200, {"running": true, "relay_list": {"source": "provisioned", "relays": ["1.2.3.4:7000"], "refresh_s": 21600, "last_error": "HTTP 503"}}]}'
run owner tunnel status
assert_contains "a real error still says failed" "$out" "Relay list: last refresh failed: HTTP 503"

echo "== A2. tickets/feedback list + activity use the passport workspace =="
ROUTES_OK='{"GET /cli/whoami": [200, {"workspace": "sales", "role": "canonical", "address": "sales", "primary": "sales", "session": "s1", "path": "'"$WORK/agentws"'"}], "GET /cli/feedback/list": [200, {"items": []}], "GET /cli/feed": [200, {"feed": []}]}'
routes "$ROUTES_OK"
run agent tickets list
assert_eq "agent tickets list exit 0" "$rc" "0"
assert_eq "agent tickets list → passport workspace" "$(last_q /cli/feedback/list)" "$WORK/agentws"
assert_eq "whoami asked with the passport" "$(last_token /cli/whoami)" "$SCOPED"
run agent feedback list
assert_eq "agent feedback list → passport workspace" "$(last_q /cli/feedback/list)" "$WORK/agentws"
run agent activity
assert_eq "agent activity exit 0" "$rc" "0"
assert_eq "agent activity → passport workspace" "$(last_q /cli/feed)" "$WORK/agentws"

routes "$ROUTES_OK"
run person tickets list
assert_eq "person tickets list → \$PWD" "$(last_q /cli/feedback/list)" "$WORK/elsewhere"
assert_absent "person never asks whoami" "$(cat "$WORK/reqs.jsonl")" "/cli/whoami"
run person activity
assert_eq "person activity → \$PWD" "$(last_q /cli/feed)" "$WORK/elsewhere"

routes "$ROUTES_OK"
run agent_pinned tickets list
assert_eq "explicit K2_PROJECT_PATH wins" "$(last_q /cli/feedback/list)" "$WORK/proj"
assert_absent "pinned: no whoami round trip" "$(cat "$WORK/reqs.jsonl")" "/cli/whoami"

routes '{"GET /cli/whoami": [400, {"error": "cannot determine identity"}], "GET /cli/feedback/list": [200, {"items": []}]}'
run agent tickets list
assert_eq "no path from whoami: exit 0" "$rc" "0"
assert_eq "no path from whoami → \$PWD fallback" "$(last_q /cli/feedback/list)" "$WORK/elsewhere"

echo
echo "CLI leftovers 0.46.3: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
