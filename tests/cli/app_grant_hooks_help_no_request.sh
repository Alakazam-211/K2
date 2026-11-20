#!/usr/bin/env bash
# -h or --help anywhere on `k2 app grant hooks` prints its usage on
# stdout, exits 0 and sends nothing.
#
# Regression (0.46.17): `_app_grant_hooks` read help only as the action,
# and the router's help guard for `k2 app grant` doesn't cover hooks, so
# `k2 app grant hooks retry <id> -h` retried the row, `skip <id> -h` skipped
# it and `reconcile -h` started a reconcile. The usage line also moves from
# stderr to stdout, as for every other help.
#
# Every help form below prints exactly the hooks usage line, nothing on
# stderr, exits 0, and the stub daemon records no request at all. Then
# the plain `retry ob-1` and `reconcile` still send their POST, so the
# stub is known to see a request when one is sent.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/app_grant_hooks_help_no_request.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-app-grant-hooks-help-XXXXXX)"
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
        self._reply({"ok": True})
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

# run <args...>: sets OUT (stdout), ERR (stderr), RC. stdin is empty, so
# a form that reads stdin can't hang.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# ref <args...>: the reference help, printed with nothing sent.
ref() {
    : >"$REQ_LOG"
    run "$@"
    REF="$OUT"
    [ "$RC" -eq 0 ] && ok "k2 $*: exit 0" || bad "k2 $*: exit $RC (want 0)"
    [ -n "$REF" ] && ok "k2 $*: prints help" || bad "k2 $*: printed nothing"
    if [ -s "$REQ_LOG" ]; then
        bad "k2 $* sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "k2 $* sends nothing"
    fi
}

# check <args...>: prints exactly REF, nothing else, sends nothing.
check() {
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$REF" ] && ok "$label: prints the help" \
        || { bad "$label: stdout is not the help"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    if [ -s "$REQ_LOG" ]; then
        bad "$label: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$label: sent nothing"
    fi
}

echo "== k2 app grant hooks: -h/--help anywhere =="
ref app grant hooks --help
[ "$REF" = "Usage: k2 app grant hooks [status|log|retry <id>|skip <id>|reconcile|target chat --on|--off]" ] \
    && ok "k2 app grant hooks --help is the hooks usage" \
    || { bad "k2 app grant hooks --help is not the hooks usage"; printf '%s\n' "$REF$ERR" | sed 's/^/      | /' >&2; }
check app grant hooks -h
check app grant hook --help
check app grant hooks retry ob-1 -h
check app grant hooks retry -h ob-1
check app grant hooks skip ob-1 --help
check app grant hooks retry -h
check app grant hooks reconcile -h
check app grant hooks reconcile --help
check app grant hooks log --subject --help
check app grant hooks log --limit -h
check app grant hooks target chat --on -h
check app grant hooks target chat --socket --help
check app grant hooks status --help
check app grant hooks --json -h

echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"
run app grant hooks retry ob-1
if grep -q '^POST /cli/skin/grant-hooks/retry {"id": "ob-1"}$' "$REQ_LOG"; then
    ok "k2 app grant hooks retry ob-1 still sends its request"
else
    bad "k2 app grant hooks retry ob-1 sent no retry"; sed 's/^/      | /' "$REQ_LOG" >&2
fi
: >"$REQ_LOG"
run app grant hooks reconcile
if grep -q '^POST /cli/skin/grant-hooks/reconcile ' "$REQ_LOG"; then
    ok "k2 app grant hooks reconcile still sends its request"
else
    bad "k2 app grant hooks reconcile sent no reconcile"; sed 's/^/      | /' "$REQ_LOG" >&2
fi

echo ""
echo "app_grant_hooks_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
