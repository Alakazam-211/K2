#!/usr/bin/env bash
# `k2 publish status` when the account's list has no row for this
# computer prints its two lines and exits 0.
#
# Regression (0.46.17): the text view ran in _connect_subdomain_render's
# exec() with only `d` in scope and ended with sys.exit(0) without
# importing sys, so after its two lines it printed a NameError traceback
# and exited 1.
#
# Rows:
#   - no tunnel subdomain on this computer: "Not connected to a K2
#     Connect subdomain." and the hint;
#   - a tunnel subdomain the list doesn't have: "Connected: <label>.k2.dev"
#     and "Plan: unknown (...)".
# Each: exactly those lines on stdout, nothing on stderr, exit 0, and one
# request (the daemon's K2 Connect proxy).
#
# How: a person's terminal outside K2 (the owner token and port on disk
# in a temp HOME) and a python stub daemon on a free loopback port that
# answers /cli/connect/proxy with the control plane's empty list. No real
# daemon, no network. Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/publish_status_no_row.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
for v in $(env | grep -E '^K[25](SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-publish-status-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        if path == "/cli/connect/proxy":
            out = {"status": 200, "body": json.dumps({"subdomains": []})}
        else:
            out = {}
        raw = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
    def do_GET(self):
        self._handle()
    def do_POST(self):
        self._handle()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj"
cat "$WORK/port" >"$HOME/.k2/heartbeat.port"
echo owner-token-for-test >"$HOME/.k2/heartbeat.token"
cd "$WORK/proj"

show() { printf '%s\n' "$1" | sed 's/^/      | /' >&2; }

# status <label> <want stdout>: k2 publish status prints exactly <want>,
# nothing on stderr, exit 0, one request to the daemon's proxy.
status() {
    local label="$1" want="$2"
    : >"$REQ_LOG"
    set +e
    OUT="$("$K2" publish status </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$want" ] && ok "$label: the two lines" \
        || { bad "$label: stdout"; show "$OUT"; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; show "$ERR"; }
    if [ "$(wc -l <"$REQ_LOG" | tr -d ' ')" = 1 ] && grep -q '^POST /cli/connect/proxy .*"/subdomains"' "$REQ_LOG"; then
        ok "$label: one request, the proxy's GET /subdomains"
    else
        bad "$label: not one proxy request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    fi
}

echo "== no tunnel subdomain on this computer =="
# native: publish status
status "publish status, not connected" "Not connected to a K2 Connect subdomain.
  Choose an address in Settings → Network → Tunnel (or run k2 tunnel start)."

echo "== a tunnel subdomain the account's list doesn't have =="
printf '{"subdomain": "acme"}\n' >"$HOME/.k2/tunnel.json"
# native: publish status
status "publish status, no row for acme" "Connected: acme.k2.dev
  Plan: unknown (subdomain not found in your account list)"

echo ""
echo "publish_status_no_row: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
