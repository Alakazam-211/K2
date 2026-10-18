#!/usr/bin/env bash
# k2 app members|room-names|self|usage (room-scope R9/R16, 0.46.1) against a
# stub daemon: the routes, query/body names, the printed answers, the
# usage errors before any call, and the owner-only switch inside K2.
# Stub daemon only. Never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*) echo "  PASS: $label"; pass=$((pass + 1)) ;;
        *) echo "  FAIL: $label (missing $(printf %q "$needle"))" >&2; fail=$((fail + 1)) ;;
    esac
}
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"; pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2; fail=$((fail + 1))
    fi
}

SANDBOX="$(mktemp -d -t k2-app-surfaces-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

export HOME="$SANDBOX"
mkdir -p "$HOME/.k2"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
for v in $(env | grep -E '^(K2|K2SO)_' | cut -d= -f1); do unset "$v"; done
export K2_HOST=127.0.0.1

STUB_PY="$SANDBOX/stub.py"
cat >"$STUB_PY" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

seen = os.environ["SEEN"]

MEMBERS = {"ok": True, "workspace": "sales", "room": "p-1", "namesHidden": False, "admins": "admins of this box", "count": 2,
           "members": [{"username": "ada", "displayName": "Ada Lovelace", "kind": "guest", "role": "staff", "app": "host", "caps": ["thread:read", "room:members"]},
                       {"username": "desk", "displayName": "desk", "kind": "agent", "role": None, "app": "jobs", "caps": ["thread:read"]}]}
SELF = {"ok": True, "app": "jobs", "workspace": "sales", "liveSeed": "ab12", "health": "running", "state": "running", "deployment": "d-1", "deployedAt": 1760000000}
USAGE = {"ok": True, "app": "jobs", "hours": 48, "quotas": {"rps": 50, "burst": 200, "sockets": 64, "bodyBytes": 1, "uploadBytes": 2, "subscriptions": 256},
         "live": {"connections": 1, "subscriptions": 0}, "totals": {"calls": 3, "denied": 1, "bytesIn": 10, "bytesOut": 20},
         "verbs": [{"verb": "thread.page", "route": "GET /cli/thread", "calls": 2, "denied": 1}, {"verb": None, "route": "GET /cli/nope", "calls": 1, "denied": 1}]}

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        raw = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _log(self, line):
        with open(seen, "a") as f:
            f.write(line + "\n")

    def do_GET(self):
        u = urlsplit(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items() if k != "token"}
        self._log("GET %s %s" % (u.path, json.dumps(q, sort_keys=True)))
        if u.path == "/cli/app/members":
            self._send(200, MEMBERS); return
        if u.path == "/cli/app/self":
            if q.get("app") == "other":
                self._send(404, {"error": {"code": "app_not_found", "hint": "no App 'other' in your workspace (k2 app list)"}}); return
            self._send(200, SELF); return
        if u.path == "/cli/app/usage":
            self._send(200, USAGE); return
        self._send(404, {"error": "not found"})

    def do_POST(self):
        u = urlsplit(self.path)
        n = int(self.headers.get("Content-Length") or "0")
        body = json.loads(self.rfile.read(n).decode() or "{}")
        self._log("POST %s %s" % (u.path, json.dumps(body, sort_keys=True)))
        if u.path == "/cli/app/room-names":
            self._send(200, {"ok": True, "workspace": body["workspace"], "room": "p-1", "namesHidden": body["hide"], "changed": True}); return
        self._send(404, {"error": "not found"})

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY
STUB_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
export K2_PORT="$STUB_PORT"
echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"
SEEN="$SANDBOX/seen.txt"
: >"$SEEN"
PORT="$STUB_PORT" SEEN="$SEEN" python3 "$STUB_PY" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if curl -s --connect-timeout 1 "http://127.0.0.1:${STUB_PORT}/" >/dev/null 2>&1; then up=1; break; fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: stub daemon never came up" >&2; exit 1; }
: >"$SEEN"

run_k2() { "$K2_CLI" "$@"; }

echo "== help =="
help_out="$(run_k2 app surfaces)"
for needle in "k2 app members [<room>]" "k2 app room-names <room> hide|show" "k2 app self <app-id>" "k2 app usage <app-id> [--hours N]"; do
    assert_contains "help: $needle" "$help_out" "$needle"
done

echo "== usage errors before any call =="
set +e
out="$(run_k2 app members 2>&1)"; rc=$?
assert_eq "members without a room outside K2: exit 2" "$rc" "2"
out="$(run_k2 app self 2>&1)"; rc=$?
assert_eq "self without an App: exit 2" "$rc" "2"
out="$(run_k2 app usage jobs --hours soon 2>&1)"; rc=$?
assert_eq "usage --hours soon: exit 2" "$rc" "2"
out="$(run_k2 app room-names sales maybe 2>&1)"; rc=$?
assert_eq "room-names needs hide|show: exit 2" "$rc" "2"
set -e
assert_eq "nothing reached the daemon yet" "$(wc -l <"$SEEN" | tr -d ' ')" "0"

echo "== members =="
out="$(run_k2 app members sales --app jobs)"
assert_contains "members header" "$out" "sales: 2 member(s)"
assert_contains "members shows the full name" "$out" "ada (Ada Lovelace)"
assert_contains "members shows an agent login" "$out" "agent"
assert_contains "members ends with the admins line" "$out" "+ admins of this box"
json="$(run_k2 app members sales --json)"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["count"]==2 and d["members"][0]["caps"]==["thread:read","room:members"], d' "$json"
echo "  PASS: members --json parses"; pass=$((pass + 1))

echo "== self and usage =="
out="$(run_k2 app self jobs)"
assert_contains "self health" "$out" "jobs (workspace sales): running"
assert_contains "self seed" "$out" "live Seed   ab12"
set +e
out="$(run_k2 app self other 2>&1)"; rc=$?
set -e
assert_eq "another workspace's App: exit 3" "$rc" "3"
assert_contains "app_not_found surfaces" "$out" "app_not_found"
out="$(run_k2 app usage jobs --hours 48)"
assert_contains "usage totals" "$out" "jobs, last 48 h: 3 calls, 1 refused"
assert_contains "usage verb line" "$out" "thread.page"
assert_contains "usage refused route" "$out" "GET /cli/nope  (1 refused)"

echo "== room-names =="
out="$(run_k2 app room-names sales hide)"
assert_contains "room-names hide" "$out" "sales: guests names hidden from Apps"
set +e
out="$(K2_HOOK_TOKEN=pp run_k2 app room-names sales show 2>&1)"; rc=$?
set -e
assert_eq "room-names inside K2 (a passport): exit 3" "$rc" "3"
assert_contains "room-names inside K2 says owner_only" "$out" "owner_only"

seen="$(cat "$SEEN")"
assert_contains "members query" "$seen" 'GET /cli/app/members {"app": "jobs", "workspace": "sales"}'
assert_contains "self query" "$seen" 'GET /cli/app/self {"app": "jobs"}'
assert_contains "usage query" "$seen" 'GET /cli/app/usage {"app": "jobs", "hours": "48"}'
assert_contains "room-names body" "$seen" 'POST /cli/app/room-names {"hide": true, "workspace": "sales"}'
case "$seen" in
    *'"hide": false'*) echo "  FAIL: room-names show went out from inside K2" >&2; fail=$((fail + 1)) ;;
    *) echo "  PASS: room-names never sent from a session"; pass=$((pass + 1)) ;;
esac

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
