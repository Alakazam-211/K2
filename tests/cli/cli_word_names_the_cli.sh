#!/usr/bin/env bash
# `k2 cli <word>`: the shell's `cli` arm names the command on both sides.
#
# The shell CLI has no `cli` verb of its own: `cli status` belongs to the
# Rust front. Its `cli)` arm prints one line on stderr and exits 2:
#
#   k2 cli status is answered by the k2 CLI binary: run k2 cli status
#
# Every name in it is the command's own, so a build under another name
# prints its own. Until 0.46.17 the line said k5 whatever the build was
# called.
#
# The name expected is the first word `version` prints, so the test holds
# under any name. Rows:
#   - `cli <a word the front has no form for>` (and with -h after it): the
#     line, exit 2, nothing on stdout, nothing sent. Both sides: the
#     binary answers it itself with the same line.
#   - `cli status`: on the shell CLI, the same line, exit 2, nothing sent.
#     On the binary it is the binary's own verb (a named difference by
#     design, crates/k5-cli/tests/front_door_parity.rs): exit 0, not the
#     line, and its one /cli/cli-usage read.
#
# How: a python stub daemon on a free loopback port records every request.
# HOME is a temp dir, no real daemon, no network. Loud: every failure is
# named and the script exits 1.
# Run with: bash tests/cli/cli_word_names_the_cli.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-cli-word-XXXXXX)"
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
import sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s\n" % (self.command, path))
        body = b'{"rows":[]}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
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

# run <args...>: sets OUT (stdout), ERR (stderr), RC.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# The command's name: the first word of what `version` prints.
run version
[ "$RC" -eq 0 ] || { echo "FAIL: version: exit $RC: $OUT$ERR" >&2; exit 1; }
NAME="${OUT%% *}"
case "$NAME" in
    ""|*[!a-z0-9-]*) echo "FAIL: version printed no command name: [$OUT]" >&2; exit 1 ;;
esac
LINE="$NAME cli status is answered by the $NAME CLI binary: run $NAME cli status"
echo "== the command is called $NAME =="

# pointer <args...>: the line on stderr, exit 2, nothing on stdout, nothing sent.
pointer() {
    local label="$NAME $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
    [ "$ERR" = "$LINE" ] && ok "$label: the line names $NAME" \
        || { bad "$label: stderr is not the line"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ -z "$OUT" ] && ok "$label: nothing on stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    if [ -s "$REQ_LOG" ]; then
        bad "$label: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$label: sent nothing"
    fi
}

echo "== cli <a word the front has no form for> =="
# native: cli <word>
pointer cli usage
pointer cli x
pointer cli Status
pointer cli ""
pointer cli status --bogus
pointer cli status --json x
pointer cli --json status
pointer cli x -h
pointer cli x --help

echo "== cli status =="
if [ -n "${K5_CLI_UNDER_TEST:-}" ]; then
    # The binary's own verb: its table, not the line.
    : >"$REQ_LOG"
    run cli status
    [ "$RC" -eq 0 ] && ok "$NAME cli status (binary): exit 0" || bad "$NAME cli status (binary): exit $RC (want 0)"
    case "$OUT$ERR" in
        *"is answered by the"*) bad "$NAME cli status (binary): printed the shell's line" ;;
        *) ok "$NAME cli status (binary): its own answer" ;;
    esac
    if grep -q '^GET /cli/cli-usage$' "$REQ_LOG"; then
        ok "$NAME cli status (binary): read /cli/cli-usage"
    else
        bad "$NAME cli status (binary): no /cli/cli-usage read"; sed 's/^/      | /' "$REQ_LOG" >&2
    fi
else
    pointer cli status
    pointer cli status --json
fi

echo ""
echo "cli_word_names_the_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
