#!/usr/bin/env bash
# -h or --help anywhere on `k2 hooks` prints the hooks help on stdout,
# exits 0 and sends nothing.
#
# Regression (0.46.17): the router's hooks arm read no help word, and
# cmd_hooks_status and cmd_hooks_install skip words they don't know, so
# `k2 hooks install -h` ran the hook installer, `uninstall -h` removed
# K2's hooks and `status -h` asked for the status. `k2 hooks -h` printed
# the usage on stderr with exit 1.
#
# Every help form below prints exactly cli/text/help/hooks.txt on stdout,
# nothing on stderr, exits 0, and the stub daemon records no request at
# all. `hooks emit -h` keeps its own page. hooks takes no bare `--` (the
# router's rule): `-h` after one is the usage error, exit 2, nothing sent.
# No verb or an unknown one is still the usage on stderr, exit 1. Help
# needs no daemon connection. Then the plain status and install still
# send, so the stub is known to see a request when one is sent.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/hooks_help_no_request.sh
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

WORK="$(mktemp -d -t k2-hooks-help-XXXXXX)"
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
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, self.path, body))
        out = json.dumps({"skipped": "agentHooks off"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def do_GET(self):
        self._handle()
    def do_POST(self):
        self._handle()
    def do_PUT(self):
        self._handle()
    def do_DELETE(self):
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

REF="$(cli_page "$TEXT/hooks.txt")"

# run <args...>: sets OUT (stdout), ERR (stderr), RC. stdin is empty.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

sent_nothing() {
    if [ -s "$REQ_LOG" ]; then
        bad "$1: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$1: sent nothing"
    fi
}

# expect <label> <rc> <stdout> <stderr>: the run just made.
expect() {
    local label="$1" rc="$2" out="$3" err="$4"
    [ "$RC" -eq "$rc" ] && ok "$label: exit $rc" || bad "$label: exit $RC (want $rc)"
    [ "$OUT" = "$out" ] && ok "$label: stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ "$ERR" = "$err" ] && ok "$label: stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
}

# check <args...>: the hooks help on stdout, nothing on stderr, exit 0,
# nothing sent.
check() {
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    expect "$label" 0 "$REF" ""
    sent_nothing "$label"
}

echo "== the hooks help =="
check hooks --help
check hooks -h
check hooks -h status
check hooks status -h
check hooks status --help
check hooks status --limit 5 -h
check hooks status --json -h
check hooks status --limit -h
check hooks install -h
check hooks install --json --help
check hooks uninstall -h
check hooks uninstall --json -h
check hooks nope -h

echo "== emit keeps its own page =="
: >"$REQ_LOG"
run hooks emit -h
expect "k2 hooks emit -h" 0 "$(cli_page "$TEXT/hooks/emit.txt")" ""
sent_nothing "k2 hooks emit -h"

echo "== hooks takes no bare --: -h after one is the usage error =="
for form in "status -- -h" "install -- -h" "uninstall -- --help"; do
    # shellcheck disable=SC2086
    : >"$REQ_LOG"; run hooks $form
    expect "k2 hooks $form" 2 "" "usage: hooks takes no '--' (k2 help hooks)"
    sent_nothing "k2 hooks $form"
done

echo "== no verb or an unknown one: the usage on stderr, exit 1 =="
: >"$REQ_LOG"; run hooks
expect "k2 hooks" 1 "" "$REF"
sent_nothing "k2 hooks"
: >"$REQ_LOG"; run hooks nope
expect "k2 hooks nope" 1 "" "$REF"
sent_nothing "k2 hooks nope"

echo "== help needs no daemon connection =="
NOHOME="$WORK/nohome"; mkdir -p "$NOHOME"
run_nodaemon() {
    set +e
    OUT="$(env -u K2_PORT -u K2_HOOK_TOKEN -u K2SO_PORT -u K2SO_HOOK_TOKEN -u K2_HOOK_SOCK \
        -u K2SO_HOOK_SOCK -u K2_HOST -u K5_PORT -u K5_HOOK_TOKEN -u K5_HOOK_SOCK -u K5_HOST \
        HOME="$NOHOME" "$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}
for form in "-h" "status -h" "install --help" "uninstall -h"; do
    # shellcheck disable=SC2086
    : >"$REQ_LOG"; run_nodaemon hooks $form
    expect "k2 hooks $form (no daemon)" 0 "$REF" ""
    sent_nothing "k2 hooks $form (no daemon)"
done
: >"$REQ_LOG"; run_nodaemon hooks install
[ "$RC" -eq 1 ] && ok "k2 hooks install (no daemon): exit 1" || bad "k2 hooks install (no daemon): exit $RC (want 1)"
[ "$(printf '%s\n' "$ERR" | head -n 1)" = "Error: Cannot connect to K2." ] && ok "k2 hooks install (no daemon): the gate stops it" \
    || { bad "k2 hooks install (no daemon): stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
sent_nothing "k2 hooks install (no daemon)"

echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"; run hooks install
grep -q '^POST /cli/hooks/install {}$' "$REQ_LOG" && ok "k2 hooks install still sends its request" \
    || { bad "install sent no request"; sed 's/^/      | /' "$REQ_LOG" >&2; }
expect "k2 hooks install" 0 "Hooks not changed: agentHooks off." ""
: >"$REQ_LOG"; run hooks uninstall
grep -q '^POST /cli/hooks/install {"remove":true}$' "$REQ_LOG" && ok "k2 hooks uninstall still sends its request" \
    || { bad "uninstall sent no request"; sed 's/^/      | /' "$REQ_LOG" >&2; }
: >"$REQ_LOG"; run hooks status --json
grep -q '^GET /cli/hooks/status?.*limit=20' "$REQ_LOG" && ok "k2 hooks status still sends its request" \
    || { bad "status sent no request"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo ""
echo "hooks_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
