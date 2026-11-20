#!/usr/bin/env bash
# -h or --help anywhere on `k2 msg` prints the msg help, exits 0 and sends
# nothing, for every form: live, --signal, the tray forms and Postal mail.
#
# Regression (0.46.17): the --signal form and cmd_signal skip words they
# don't know, so `k2 msg <ws> --signal <kind> --help` sent the signal, and
# a value flag (--signal, --payload, --from, --title) took --help as its
# value. The live form already printed the help; now every form prints
# that same text before anything else runs.
#
# Every help form below prints exactly what `k2 msg --help` prints, on
# stdout, nothing on stderr, exits 0, and the stub daemon records no
# request at all. Then the plain `--signal` form still sends its POST, so
# the stub is known to see a request when one is sent.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/msg_help_no_request.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-msg-help-XXXXXX)"
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

echo "== k2 msg: -h/--help anywhere =="
ref msg --help
case "$REF" in
    "k2 msg — talk to another workspace agent"*) ok "k2 msg --help is the msg help" ;;
    *) bad "k2 msg --help is not the msg help"; printf '%s\n' "$REF" | sed 's/^/      | /' >&2 ;;
esac
check msg -h
check msg acme hello -h
echo "== the --signal form =="
check msg acme --signal msg --help
check msg acme --signal msg -h
check msg acme --signal --help
check msg acme -h --signal msg
check msg acme --signal status --payload '{"text":"x"}' -h
check msg acme --signal msg --payload --help
check msg acme --signal msg --from --help
check msg acme --signal msg --inbox -h
echo "== the tray forms =="
check msg acme --inbox-wake -h
check msg acme --inbox-silent --title --help notes.txt
check msg acme --inbox -h
check msg acme --inbox-silent --inbox-wake --help
echo "== Postal mail =="
check msg ada::acme-box.postal.bot hello -h

echo "== the stub sees the plain form's request =="
: >"$REQ_LOG"
run msg acme --signal msg --payload '{"text":"hi"}'
if grep -q '^POST /cli/awareness/publish ' "$REQ_LOG"; then
    ok "k2 msg acme --signal msg still sends its signal"
else
    bad "k2 msg acme --signal msg sent no signal"; sed 's/^/      | /' "$REQ_LOG" >&2
fi

echo ""
echo "msg_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
