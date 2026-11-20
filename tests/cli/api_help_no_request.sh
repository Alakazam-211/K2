#!/usr/bin/env bash
# -h or --help anywhere on `k2 api` and `k2 api-key` prints help, exits 0
# and sends nothing.
#
# Regression (0.46.17): cmd_api and cmd_api_key never read -h/--help, so
# `k2 api on -h` turned the public /v1 API on and `k2 api-key revoke <id>
# -h` revoked the key. Now every form below prints the family's usage on
# stdout (the same text an unknown verb prints on stderr), nothing on
# stderr, exits 0, and the stub daemon records no request at all.
#
# Then the plain forms (`k2 api on`, `k2 api-key revoke <id>`) still send
# their POST, so the stub is known to see a request when one is sent.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/api_help_no_request.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-api-help-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _reply(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        if path == "/cli/settings/update":
            return self._reply({"apiEnabled": json.loads(body).get("apiEnabled")})
        if path.startswith("/cli/api-keys/"):
            return self._reply({"success": True, "keys": []})
        self._reply({})
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
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN="owner-token-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

# run <args...>: sets OUT (stdout), ERR (stderr), RC. stdin is empty, so a
# form that would read a key from stdin can't hang.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# The usage each family prints for an unknown verb (on stderr, exit 2 or
# 1). Help prints exactly this text on stdout.
: >"$REQ_LOG"
run api no-such-verb
API_USAGE="$ERR"
[ "$RC" -eq 2 ] && ok "k2 api no-such-verb: exit 2" || bad "k2 api no-such-verb: exit $RC (want 2)"
case "$API_USAGE" in
    "Usage: k2 api <on|off|status>"*) ok "k2 api no-such-verb prints the api usage" ;;
    *) bad "k2 api no-such-verb: no usage line"; printf '%s\n' "$OUT$ERR" | sed 's/^/      | /' >&2 ;;
esac
run api-key no-such-verb
KEY_USAGE="$ERR"
[ "$RC" -eq 1 ] && ok "k2 api-key no-such-verb: exit 1" || bad "k2 api-key no-such-verb: exit $RC (want 1)"
case "$KEY_USAGE" in
    "Usage: k2 api-key <create|list|hire-ceiling|revoke|disable|enable>"*) ok "k2 api-key no-such-verb prints the api-key usage" ;;
    *) bad "k2 api-key no-such-verb: no usage line"; printf '%s\n' "$OUT$ERR" | sed 's/^/      | /' >&2 ;;
esac
if [ -s "$REQ_LOG" ]; then
    bad "an unknown verb sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
else
    ok "an unknown verb sends nothing"
fi

# check <want-usage> <args...>
check() {
    local want="$1"; shift
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$want" ] && ok "$label: prints the usage" \
        || { bad "$label: stdout is not the usage"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    if [ -s "$REQ_LOG" ]; then
        bad "$label: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$label: sent nothing"
    fi
}

echo "== k2 api: -h/--help anywhere =="
check "$API_USAGE" api -h
check "$API_USAGE" api --help
check "$API_USAGE" api on -h
check "$API_USAGE" api on --help
check "$API_USAGE" api off -h
check "$API_USAGE" api off --help
check "$API_USAGE" api -h on
check "$API_USAGE" api --help off
check "$API_USAGE" api status -h
check "$API_USAGE" api status --help

echo "== k2 api-key: -h/--help anywhere =="
check "$KEY_USAGE" api-key -h
check "$KEY_USAGE" api-key --help
check "$KEY_USAGE" api-key revoke X -h
check "$KEY_USAGE" api-key revoke X --help
check "$KEY_USAGE" api-key -h revoke X
check "$KEY_USAGE" api-key revoke -h
check "$KEY_USAGE" api-key disable X -h
check "$KEY_USAGE" api-key enable X --help
check "$KEY_USAGE" api-key list -h
check "$KEY_USAGE" api-key create --label L -h
check "$KEY_USAGE" api-key create --status --help
check "$KEY_USAGE" api-key create --label L --workspaces '*' --hire --help
check "$KEY_USAGE" api-key hire-ceiling X --none -h
check "$KEY_USAGE" api-key app-access X --help

echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"
run api on
if grep -q '^POST /cli/settings/update {"apiEnabled":true}$' "$REQ_LOG"; then
    ok "k2 api on still sends its request"
else
    bad "k2 api on sent no settings update"; sed 's/^/      | /' "$REQ_LOG" >&2
fi
[ "$RC" -eq 0 ] && ok "k2 api on: exit 0" || bad "k2 api on: exit $RC (want 0)"
case "$OUT" in
    *"Public /v1 API setting: ON"*) ok "k2 api on reports ON" ;;
    *) bad "k2 api on: no ON line"; printf '%s\n' "$OUT$ERR" | sed 's/^/      | /' >&2 ;;
esac
: >"$REQ_LOG"
run api-key revoke X
if grep -q '^POST /cli/api-keys/revoke {"id": "X"}$' "$REQ_LOG"; then
    ok "k2 api-key revoke X still sends its request"
else
    bad "k2 api-key revoke X sent no revoke"; sed 's/^/      | /' "$REQ_LOG" >&2
fi
[ "$RC" -eq 0 ] && ok "k2 api-key revoke X: exit 0" || bad "k2 api-key revoke X: exit $RC (want 0)"

echo ""
echo "api_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
