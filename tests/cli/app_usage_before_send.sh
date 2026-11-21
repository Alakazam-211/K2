#!/usr/bin/env bash
# `k2 app` verbs check every word before they send anything: a usage error
# or a help line makes no request, and nothing is half done.
#
# Regression (0.46.17):
#   - `k2 app access <ws> --build on --users maybe` turned "Build and run
#     Apps" on, then stopped on --users with a usage error: --users was
#     checked only after the first switch was sent.
#
# Each refusal row: the exact line on stderr, nothing on stdout, exit 2,
# and the stub daemon records no request. Each row that should send then
# sends exactly what it should, so the stub is known to see a request.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/app_usage_before_send.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
TEXT="$SCRIPT_DIR/../../cli/text/help"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-app-usage-XXXXXX)"
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
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        if path == "/cli/agent-access/set":
            out = {"workspace": "ws", "ok": True}
        elif path == "/cli/app/list":
            out = {"access": {"workspace": "ws", "buildAndRun": True, "manageEndToEnd": False,
                              "manageUsers": True, "coreList": []}}
        elif path == "/cli/app/key/revoke":
            out = {"revoked": True, "id": json.loads(body).get("id"), "app": {"handle": "shop"}}
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
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN="owner-token-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

run() {
    : >"$REQ_LOG"
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

show() { printf '%s\n' "$1" | sed 's/^/      | /' >&2; }

sent_nothing() {
    if [ -s "$REQ_LOG" ]; then
        bad "$1: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$1: sent nothing"
    fi
}

# refused <stderr> <args...>: exactly <stderr> on stderr, nothing on
# stdout, exit 2, nothing sent.
refused() {
    local want="$1"; shift
    local label="k2 $*"
    run "$@"
    [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
    [ "$ERR" = "$want" ] && ok "$label: the usage line" \
        || { bad "$label: stderr"; show "$ERR"; }
    [ -z "$OUT" ] && ok "$label: nothing on stdout" \
        || { bad "$label: stdout"; show "$OUT"; }
    sent_nothing "$label"
}

# page <file> <args...>: exactly cli/text/help/<file> on stdout, nothing
# on stderr, exit 0, nothing sent.
page() {
    local file="$1"; shift
    local label="k2 $*" want
    want="$(cat "$TEXT/$file")"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$want" ] && ok "$label: prints $file" \
        || { bad "$label: stdout is not $file"; show "$OUT"; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; show "$ERR"; }
    sent_nothing "$label"
}

# sent <n> <pattern>: the stub saw exactly <n> requests and line <n>
# matches <pattern> (grep -E).
sent_line() {
    local label="$1" n="$2" pat="$3"
    if sed -n "${n}p" "$REQ_LOG" | grep -Eq -- "$pat"; then
        ok "$label: request $n is $pat"
    else
        bad "$label: request $n is not $pat"; sed 's/^/      | /' "$REQ_LOG" >&2
    fi
}

echo "== app access: every value is checked before the first switch is sent =="
U_BUILD="usage: --build on|off"
U_MANAGE="usage: --manage on|off"
U_USERS="usage: --users on|off"
U_CORE="usage: --core <core>=on|off[,<core>=on|off…] (cores: db, files, thread, mail, sew)"
# native: app access
refused "$U_USERS" app access ws --build on --users maybe
refused "$U_USERS" app access ws --users maybe
refused "$U_USERS" app access ws --build off --users maybe
refused "$U_USERS" app access ws --build on --manage on --users yes
refused "$U_USERS" app access ws --manage on --users maybe
refused "$U_USERS" app access ws --users ON --core db=on
refused "$U_USERS" app access ws --users maybe --json
refused "$U_BUILD" app access ws --build maybe --users on
refused "$U_BUILD" app access ws --users on --build 1
refused "$U_MANAGE" app access ws --build on --manage maybe
refused "$U_MANAGE" app access ws --users on --manage yes
refused "$U_CORE" app access ws --build on --core db=maybe
refused "$U_CORE" app access ws --users on --core db
refused "Unknown flag: --bogus" app access ws --build on --bogus

echo "== app access: good values still send, in order =="
# native: app access
run app access ws --build on --users on
[ "$RC" -eq 0 ] && ok "k2 app access ws --build on --users on: exit 0" \
    || { bad "k2 app access ws --build on --users on: exit $RC"; show "$ERR"; }
sent_line "the build switch" 1 '^POST /cli/agent-access/set .*"toggle": "apps_build".*"value": true'
sent_line "the users switch" 2 '^POST /cli/agent-access/set .*"toggle": "app_users".*"value": true'
sent_line "the read back" 3 '^GET /cli/app/list '
[ "$(wc -l <"$REQ_LOG" | tr -d ' ')" = 3 ] && ok "three requests in all" \
    || { bad "not three requests"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo ""
echo "app_usage_before_send: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
