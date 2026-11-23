#!/usr/bin/env bash
# k2 ci never prints a Python traceback: an answer its reader can't read
# is one plain line on stderr and exit 1.
#
# Regression (0.46.17): the dashdash sweep's stub answers every request
# with a plain 200, {"ok": true}. `k2 ci init -- x`, `run -- x`, `stats
# -- x`, `monitors -- x` and `tests -- x` (and each with `-- -h`) sent
# their request, and the reader raised on the answer (KeyError,
# TypeError): a Python traceback reached the person or agent, worded
# differently on each Python version. Now every ci reader of a daemon
# answer starts with _CI_PY_PLAIN (cli/src/ci.sh):
#   bad_response: k2 ci can't read the daemon's answer (a daemon on another K2 version?)
# for an answer it can't read, `io_error: <the system's words>` for a
# file it could not write, and exit 1 either way. What a reader printed
# before it raised stays on stdout.
#
# How: a python stub daemon on a free loopback port answers {"ok": true}
# to every request but two: /cli/ci/status answers a body that is not
# JSON, and /cli/ci/setup a setup with nothing missing (so `setup --out`
# writes its file). HOME is a temp dir, no real daemon, no network. Loud.
# Run with: bash tests/cli/ci_plain_errors.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-ci-plain-XXXXXX)"
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
        if path == "/cli/ci/status":
            out = b"nope"
        elif path == "/cli/ci/setup":
            out = json.dumps({"missing": [], "setup": {"a": 1}}).encode()
        else:
            out = json.dumps({"ok": True}).encode()
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
# The owner's own terminal: the port and token come from HOME/.k2.
unset K2_PORT K2_HOOK_TOKEN
mkdir -p "$HOME/.k2"
cat "$WORK/port" >"$HOME/.k2/heartbeat.port"
printf 'owner-token-for-test\n' >"$HOME/.k2/heartbeat.token"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

BAD="bad_response: k2 ci can't read the daemon's answer (a daemon on another K2 version?)"

# run <args...>: sets OUT (stdout), ERR (stderr), RC. stdin is empty.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# plain <want stdout> <want stderr> <route> <args...>: exit 1, exactly
# that stdout and stderr, and the request went to <route>.
plain() {
    local want_out="$1" want_err="$2" route="$3"
    shift 3
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 1 ] && ok "$label: exit 1" || bad "$label: exit $RC (want 1)"
    [ "$ERR" = "$want_err" ] && ok "$label: one plain line on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    case "$ERR" in
        *Traceback*) bad "$label: a traceback on stderr" ;;
        *) ok "$label: no traceback" ;;
    esac
    [ "$OUT" = "$want_out" ] && ok "$label: stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    if grep -q "^[A-Z]* $route " "$REQ_LOG"; then
        ok "$label: asked $route"
    else
        bad "$label: never asked $route"; sed 's/^/      | /' "$REQ_LOG" >&2
    fi
}

echo "== a 200 that is not the route's answer, after a bare -- =="
for tail in "x-k5-sweep" "-h"; do
    plain "" "$BAD" /cli/ci/init ci init -- "$tail"
    plain "" "$BAD" /cli/ci/run ci run -- "$tail"
    plain "" "$BAD" /cli/ci/stats ci stats -- "$tail"
    plain "" "$BAD" /cli/ci/monitors ci monitors -- "$tail"
    plain "" "$BAD" /cli/ci/tests ci tests -- "$tail"
done

echo "== the same answers without the -- =="
plain "" "$BAD" /cli/ci/run ci run
plain "" "$BAD" /cli/ci/stats ci stats

echo "== a body that is not JSON =="
plain "" "$BAD" /cli/ci/status ci status

echo "== what a reader printed before it raised stays on stdout =="
plain "this server (controller): None" "$BAD" /cli/ci/signers ci signer list

echo "== a file it can't write: io_error, the system's words =="
OUTF="$WORK/no-such-dir/setup.json"
plain "" "io_error: [Errno 2] No such file or directory: '$OUTF'
can't write $OUTF" /cli/ci/setup ci setup --plan --out "$OUTF"

echo ""
echo "ci_plain_errors: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
