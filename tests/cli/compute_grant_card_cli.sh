#!/usr/bin/env bash
# k2 compute grant and the compute.grant card (0.46.17): CLI shapes against
# a tiny stub daemon on loopback (python3 http.server). No real daemon, no
# ~/.k2: HOME is a throwaway dir.
#
# Covers: an agent's `k2 compute grant <node> --max-time 4h` sends the
# limits it typed (seconds) and, refused owner_only with the daemon's
# permission object, exits 3 and ends with "Ask with: k2 permission request
# compute.grant max-time=4h node=…"; in an agent's session --workspace may
# be left out (the body names none: the passport's own), while a person's
# run without it is a usage error with no request; the help, `k2 study
# compute` and `k2 --schema` name the card. (The permissions skill's
# section is checked in k2-core: it equals `k2 study permissions`.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2cg-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

ASK_TIME='k2 permission request compute.grant max-time=4h node=k2n15 --why "<what you need it for>"'
ASK_MANY='k2 permission request compute.grant max-disk=120 max-parallel=2 max-queued=20 max-time=1h30m node=k2n15 --why "<what you need it for>"'

# The stub answers as the daemon does for an agent's passport with cards on:
# owner_only plus the permission object the walls map builds (its command
# from the body's values; the daemon's own test pins that mapping).
cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]

def word(secs):
    h, m, s = secs // 3600, (secs % 3600) // 60, secs % 60
    return "".join("%d%s" % (n, u) for n, u in ((h, "h"), (m, "m"), (s, "s")) if n) or "0s"

def ask(body):
    params = {"node": body.get("node")}
    for key, k in (("max-time", "maxJobSecs"), ("max-parallel", "maxParallel"), ("max-queued", "maxQueued"), ("max-disk", "maxDiskGb")):
        if k in body:
            params[key] = word(body[k]) if key == "max-time" else str(body[k])
    cmd = "k2 permission request compute.grant" + "".join(" %s=%s" % (k, params[k]) for k in sorted(params))
    return {"action": "compute.grant", "params": params, "approvers": "owner or admin", "command": cmd + ' --why "<what you need it for>"'}

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
        if u.path != "/cli/compute/grants/set":
            return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
        return self.reply(403, {"ok": False, "error": {"code": "owner_only", "hint": "only the server's owner or an admin"}, "permission": ask(body)})

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
    err="$(K2_PORT="$PORT" K2_HOOK_TOKEN=passport-builder "$K2" compute grant "$@" 2>&1 </dev/null >/dev/null)"; rc=$?
    set -e
}

echo "== an agent's own limits: exit 3, ends with the ask =="
agent k2n15 --max-time 4h --workspace builder
expect_eq "exits 3" "$rc" "3"
expect_has "refusal code" "$err" "owner_only"
expect_eq "it ends with the ask" "$(printf '%s\n' "$err" | tail -1)" "Ask with: $ASK_TIME"
expect_eq "the body carries what was typed" "$(last body)" '{"maxJobSecs": 14400, "node": "k2n15", "workspace": "builder"}'
expect_eq "sent with the passport" "$(last token)" "passport-builder"

agent k2n15 --max-time 1h30m --max-parallel 2 --max-queued 20 --max-disk 120G --workspace builder
expect_eq "every limit: exits 3" "$rc" "3"
expect_eq "every limit: ends with the ask" "$(printf '%s\n' "$err" | tail -1)" "Ask with: $ASK_MANY"
expect_eq "every limit: body" "$(last body)" '{"maxDiskGb": 120, "maxJobSecs": 5400, "maxParallel": 2, "maxQueued": 20, "node": "k2n15", "workspace": "builder"}'

echo "== --workspace: an agent may leave it out, a person may not =="
agent k2n15 --max-time 4h
expect_eq "no --workspace in a session: exits 3 (asked)" "$rc" "3"
expect_eq "no --workspace in a session: the ask" "$(printf '%s\n' "$err" | tail -1)" "Ask with: $ASK_TIME"
expect_eq "no --workspace in a session: the body names none" "$(last body)" '{"maxJobSecs": 14400, "node": "k2n15"}'
n_before="$(wc -l < "$LOG")"
set +e
err="$("$K2" compute grant k2n15 --max-time 4h 2>&1 </dev/null >/dev/null)"; rc=$?
set -e
expect_eq "a person without --workspace: usage, exit 2" "$rc" "2"
expect_has "a person without --workspace: the usage line" "$err" "usage: k2 compute grant <node> --workspace WS"
expect_eq "a person without --workspace: no request" "$(wc -l < "$LOG")" "$n_before"

echo "== help, study and schema name the card =="
help="$("$K2" compute --help)"
expect_has "help: the card" "$help" "compute.grant node=<node> max-time=4h"
expect_has "help: exactly those" "$help" "Allow sets exactly those; the others stay as they are"
study="$("$K2" study compute)"
expect_has "study: the section" "$study" "GRANT LIMITS (0.46.17)"
expect_has "study: the card" "$study" 'k2 permission request compute.grant node=<node> max-time=4h --why "…"'
expect_has "study: --ci and --gpu stay their own" "$study" "--ci and --gpu"
schema="$("$K2" --schema)"
expect_has "schema: the card" "$schema" "k2 permission request compute.grant node=<node>"

echo
echo "compute grant card CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
