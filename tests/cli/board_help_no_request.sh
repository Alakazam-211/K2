#!/usr/bin/env bash
# -h or --help anywhere on `k2 board` prints the board help, exits 0 and
# sends nothing, for a Garden board and a .k5board file alike.
#
# Regression (0.46.17): cmd_board looked for help in the first word only,
# and its parser reads `--help` as a field, so `k2 board add --help` added
# a sticky with a field "help" (and `k2 board delete <id> -h` deleted it).
# Now every form below prints exactly what `k2 board --help` prints, on
# stdout, nothing on stderr, exits 0, and the stub daemon records no
# request at all.
#
# Then the plain forms (`k2 board add sticky ...` on a Garden board and on
# a .k5board file) still send their POST, so the stub is known to see a
# request when one is sent.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/board_help_no_request.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-board-help-XXXXXX)"
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
        out = json.dumps({"ok": True, "board": "b1a2b3c", "rev": 7,
                          "object": {"id": "s-0123456789", "type": "sticky"}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
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
# `apply -` can't hang.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

: >"$REQ_LOG"
run board --help
REF="$OUT"
[ "$RC" -eq 0 ] && ok "k2 board --help: exit 0" || bad "k2 board --help: exit $RC (want 0)"
case "$REF" in
    "Usage: k2 board <command> [args] [--board <id|name>] [--json]"*) ok "k2 board --help prints the board help" ;;
    *) bad "k2 board --help: no usage line"; printf '%s\n' "$OUT$ERR" | sed 's/^/      | /' >&2 ;;
esac
if [ -s "$REQ_LOG" ]; then
    bad "k2 board --help sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
else
    ok "k2 board --help sends nothing"
fi

# check <args...>
check() {
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$REF" ] && ok "$label: prints the board help" \
        || { bad "$label: stdout is not the board help"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    if [ -s "$REQ_LOG" ]; then
        bad "$label: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$label: sent nothing"
    fi
}

echo "== a Garden board: -h/--help anywhere =="
check board -h
check board help
check board add --help
check board add -h
check board add sticky --text hi --help
check board add sticky --help --text hi
check board add sticky --text hi --board Launch -h
check board frame --title F --help
check board update s-1 --text x -h
check board delete s-1 --help
check board rm s-1 -h
check board connect a b -h
check board react s-1 up --help
check board vote s-1 -h
check board comment s-1 hello --help
check board meta --title T -h
check board template retro --help
check board apply plan.json -h
check board board-new --name N --help
check board board-rename --name N -h
check board list --help

echo "== a .k5board file: -h/--help anywhere =="
check board add sticky --text hi --help plan.k5board
check board new plan.k5board -h
check board delete s-1 --help plan.k5board
check board --file plan.k5board update s-1 --text x -h
check board apply - plan.k5board --help

echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"
run board add sticky --text hi --board Launch
if grep -q '^POST /cli/zen/data/board .*"cmd": "add"' "$REQ_LOG"; then
    ok "k2 board add (Garden board) still sends its request"
else
    bad "k2 board add (Garden board) sent no add"; sed 's/^/      | /' "$REQ_LOG" >&2
fi
: >"$REQ_LOG"
run board add sticky --text hi plan.k5board
if grep -q '^POST /cli/board/file/verb ' "$REQ_LOG"; then
    ok "k2 board add (.k5board file) still sends its request"
else
    bad "k2 board add (.k5board file) sent no verb"; sed 's/^/      | /' "$REQ_LOG" >&2
fi

echo ""
echo "board_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
