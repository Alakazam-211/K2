#!/usr/bin/env bash
# `k2 workspace remove <ws> --apps <way>` answers /cli/app/release the way
# `k2 app release` does.
#
# A stub daemon on loopback answers the release by its way:
#   park             → 200, the Apps released: the workspace is removed (exit 0)
#   handover:globex  → 202, one handover card held for the owner: the
#                      daemon's hint, then a line that the workspace is not
#                      removed until the owner approves (exit 2); no removal
#                      is sent
#   uninstall        → 403 owner_only: the refusal line (exit 3); no removal
#
# Before the fix a 202 printed {"error": "error", "hint": "None"} and
# exited 3.
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is unset, stdin
# is /dev/null (no terminal, so nothing is approved here), no real daemon.
# Run with: bash tests/cli/workspace_remove_apps.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K[25](SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; diff <(printf '%s\n' "$3") <(printf '%s\n' "$2") >&2 || true; fi; }

SANDBOX="$(mktemp -d /tmp/k2wsra-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
HELD = {
    "ok": True, "status": "held", "workspace": "acme", "way": "handover:globex",
    "results": [{"app": "jobs", "ok": True, "status": "held", "request": "r_held1", "handover": "h_1"}],
    "hint": "1 handover card(s) wait for the owner's approval (k2 app approve <request> from a terminal outside K2, "
            "or Settings → Cores → App Approvals); the workspace keeps its Apps until then",
}

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def reply(self, st, obj):
        body = json.dumps(obj).encode()
        self.send_response(st)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode() if n else ""
        path = self.path.split("?", 1)[0]
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s\n" % (self.command, path))
        if path == "/cli/workspace/resolve":
            return self.reply(200, {"path": "/srv/acme"})
        if path == "/cli/workspace/remove":
            return self.reply(200, {"success": True})
        if path == "/cli/app/release":
            way = json.loads(raw).get("apps")
            if way == "park":
                return self.reply(200, {"ok": True, "workspace": "acme", "way": "park", "results": [{"app": "jobs", "ok": True}]})
            if way == "handover:globex":
                return self.reply(202, HELD)
            return self.reply(403, {"error": {"code": "owner_only", "hint": "only the owner may release this workspace's Apps"}})
        self.reply(404, {"error": {"code": "not_found", "hint": "no such route"}})
    do_POST = do_GET

srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$SANDBOX/stub.py" "$SANDBOX" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"
export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
cd "$SANDBOX/ws"

# rc_of <args…>: an agent in a K2 terminal over TCP; stdout, stderr and the
# routes asked land in the sandbox.
rc_of() {
    : >"$SANDBOX/requests.log"
    local rc=0
    K2_PORT="$PORT" K2_HOOK_TOKEN=passport-wsra "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err" || rc=$?
    echo "$rc"
}

echo "== 200: the Apps released, then the workspace removed =="
rc="$(rc_of workspace remove acme --apps park)"
expect_eq "200: exit 0" "$rc" "0"
expect_eq "200: resolve, release, then the removal" "$(cat "$SANDBOX/requests.log")" \
'GET /cli/workspace/resolve
POST /cli/app/release
POST /cli/workspace/remove'
expect_eq "200: nothing on stderr" "$(cat "$SANDBOX/err")" ""

echo "== 202: a handover held for the owner; the removal waits =="
rc="$(rc_of workspace remove acme --apps handover:globex)"
expect_eq "202: exit 2 (held), as k2 app release" "$rc" "2"
expect_eq "202: the daemon's hint, then the workspace is not removed" "$(cat "$SANDBOX/out")" \
"1 handover card(s) wait for the owner's approval (k2 app approve <request> from a terminal outside K2, or Settings → Cores → App Approvals); the workspace keeps its Apps until then
The workspace is not removed: it keeps its Apps until the owner approves the handover. Once they have, run the same command again."
expect_eq "202: nothing on stderr" "$(cat "$SANDBOX/err")" ""
expect_eq "202: no removal is sent" "$(cat "$SANDBOX/requests.log")" \
'GET /cli/workspace/resolve
POST /cli/app/release'

echo "== A refusal: nothing removed =="
rc="$(rc_of workspace remove acme --apps uninstall)"
expect_eq "refusal: exit 3" "$rc" "3"
expect_eq "refusal: the refusal line" "$(sed -n 1p "$SANDBOX/err")" \
'{"error": "owner_only", "hint": "only the owner may release this workspace'"'"'s Apps"}'
expect_eq "refusal: no removal is sent" "$(cat "$SANDBOX/requests.log")" \
'GET /cli/workspace/resolve
POST /cli/app/release'

echo
echo "workspace_remove_apps: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
