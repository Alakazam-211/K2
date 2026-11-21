#!/usr/bin/env bash
# A value flag never takes -h or --help as its value: help wins, on every
# `k2 app` verb that changes something. Help prints its page on stdout,
# exits 0, and the stub daemon records no request.
#
# Regression (0.46.17):
#   - `k2 app data put <app> <path> --to -h` sent the put with `to` set
#     to `-h`.
#   - `k2 app key mint <app> --scopes u --label -h` sent the mint with
#     the label `-h`.
#
# Each help row: exactly the page on stdout, nothing on stderr, exit 0,
# nothing sent. Each verb's plain form then still sends, so the stub is
# known to see a request.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/app_value_flag_help.sh
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

WORK="$(mktemp -d -t k2-app-value-help-XXXXXX)"
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
        elif path == "/cli/app/key/mint":
            out = {"id": "k3", "app": {"handle": "shop"}, "scopes": ["users"], "routes": "/v1/users"}
        elif path == "/cli/app/data/put":
            out = {"files": 1, "from": "/x", "to": "/", "app": "shop"}
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

echo "== app data put: a value flag never takes -h =="
# native: app data put
page app/data.txt app data put shop notes.txt --to -h
# native: app data put
page app/data.txt app data put shop notes.txt --to --help
# native: app data put
page app/data.txt app data put shop --to -h notes.txt
# native: app data put
page app/data.txt app data put shop notes.txt --disk-override -h
# native: app data put
page app/data.txt app data put shop notes.txt --disk-override=10G --to --help
echo "== app data put: the plain put still sends =="
# native: app data put
run app data put shop notes.txt --to sub
[ "$RC" -eq 0 ] && ok "k2 app data put shop notes.txt --to sub: exit 0" || { bad "data put: exit $RC"; show "$ERR"; }
sent_line "the put" 1 '^POST /cli/app/data/put .*"to": "sub"'
[ "$(wc -l <"$REQ_LOG" | tr -d ' ')" = 1 ] && ok "data put: one request" \
    || { bad "data put: not one request"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo "== app key mint: a value flag never takes -h =="
# native: app key mint
page app/grace.txt app key mint shop --scopes u --label -h
# native: app key mint
page app/grace.txt app key mint shop --scopes -h
# native: app key mint
page app/grace.txt app key mint shop --scope users --rooms --help
# native: app key mint
page app/grace.txt app key mint shop --scopes users --roles -h --show
# native: app key mint
page app/grace.txt app key mint --label --help shop --scopes users
echo "== app key mint: the plain mint still sends =="
# native: app key mint
run app key mint shop --scopes users --label L
[ "$RC" -eq 0 ] && ok "k2 app key mint shop --scopes users --label L: exit 0" || { bad "key mint: exit $RC"; show "$ERR"; }
sent_line "the mint" 1 '^POST /cli/app/key/mint .*"label": "L"'
[ "$(wc -l <"$REQ_LOG" | tr -d ' ')" = 1 ] && ok "key mint: one request" \
    || { bad "key mint: not one request"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo ""
echo "app_value_flag_help: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
