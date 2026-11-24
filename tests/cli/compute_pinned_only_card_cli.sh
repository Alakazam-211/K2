#!/usr/bin/env bash
# k2 compute node pinned-only and the compute.node.pinned-only card
# (0.46.18): CLI shapes against a tiny stub daemon on loopback (python3
# http.server). No real daemon, no ~/.k2: HOME is a throwaway dir.
#
# Covers: an agent's `k2 compute node pinned-only <node> on|off` sends the
# node and `on` as a JSON boolean and, refused owner_only with the daemon's
# permission object, exits 3 and ends with "Ask with: k2 permission request
# compute.node.pinned-only node=<node> on=true|false" (never
# compute.manage); a bad mode is a usage error with no request; `k2 study
# permissions` lists the card. (The daemon's own tests pin the mapping
# from the refused body to the line; the permissions skill equals the
# study page, checked in k2-core.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2po-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

# The stub answers as the daemon does for an agent's passport with cards on:
# owner_only plus the permission object the walls map builds from the body.
cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]

def ask(body):
    on = "true" if body.get("on") is True else "false"
    params = {"node": body.get("node"), "on": on}
    cmd = "k2 permission request compute.node.pinned-only node=%s on=%s" % (params["node"], on)
    return {"action": "compute.node.pinned-only", "params": params, "approvers": "owner or admin", "command": cmd + ' --why "<what you need it for>"'}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        token = (parse_qs(u.query).get("token") or [""])[0] or (self.headers.get("Authorization") or "").replace("Bearer ", "")
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "body": body, "token": token}) + "\n")
        if u.path != "/cli/compute/nodes/pinned-only":
            return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
        return self.reply(403, {"error": {"code": "owner_only", "hint": "only the server's owner (or an admin, where noted) can do this"}, "permission": ask(body)})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"

last() { tail -1 "$LOG" | python3 -c "import json,sys; v=json.load(sys.stdin); x=v[sys.argv[1]]; print(x if isinstance(x, str) else json.dumps(x, sort_keys=True))" "$1"; }

agent() {
    set +e
    err="$(K2_PORT="$PORT" K2_HOOK_TOKEN=passport-builder "$K2" compute node pinned-only "$@" 2>&1 </dev/null >/dev/null)"; rc=$?
    set -e
}

for mode in on off; do
    word=true; [ "$mode" = off ] && word=false
    echo "== an agent's pinned-only $mode: exit 3, ends with the card's line =="
    agent node-5 "$mode"
    expect_eq "$mode: exits 3" "$rc" "3"
    expect_has "$mode: refusal code" "$err" "owner_only"
    expect_eq "$mode: it ends with the ask" "$(printf '%s\n' "$err" | tail -1)" "Ask with: k2 permission request compute.node.pinned-only node=node-5 on=$word --why \"<what you need it for>\""
    if grep -Fq "compute.manage" <<<"$err"; then bad "$mode: never compute.manage"; else ok "$mode: never compute.manage"; fi
    expect_eq "$mode: the body" "$(last body)" "{\"node\": \"node-5\", \"on\": $word}"
    expect_eq "$mode: sent with the passport" "$(last token)" "passport-builder"
done

echo "== a bad mode: usage, no request =="
n_before="$(wc -l < "$LOG")"
agent node-5 maybe
expect_eq "bad mode: exit 2" "$rc" "2"
expect_has "bad mode: the usage line" "$err" "usage: k2 compute node pinned-only <name> on|off"
expect_eq "bad mode: no request" "$(wc -l < "$LOG")" "$n_before"

echo "== the study page lists the card =="
# The page is reached only while cards are on; the bash function holds it.
page="$(sed -n '/^_study_page_permissions() {$/,/^}$/p' "$CLI_BASH_SOURCE")"
expect_has "study: the section" "$page" "## A compute node's pinned jobs only mode"
expect_has "study: the card" "$page" 'k2 permission request compute.node.pinned-only node=<node> on=true --why "<what you need it for>"'

echo
echo "compute pinned-only card CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
