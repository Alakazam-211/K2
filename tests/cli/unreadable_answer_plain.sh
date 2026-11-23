#!/usr/bin/env bash
# A doc, organization or compute reader that can't read the daemon's
# answer prints that family's plain error and exits 1, never a Python
# traceback.
#
# Regression (0.46.17): the dashdash sweep's stub answers every request
# with a plain 200, {"ok": true}. `k2 doc status|flush|open|leave|lease|
# lock|unlock|watch -- x`, `k2 organization bulletin set -- x` and
# `delete -- x`, and `k2 compute run|test -- x` sent their request and the
# reader raised on the answer (KeyError): a traceback reached the person
# or agent, worded differently on each Python version. Now each of those
# readers prints the family's own error:
#   doc           _doc_die's two lines, code bad_response
#                 (_DOC_PY_PLAIN, cli/src/doc.sh: every _doc_print_json_or
#                 reader and the watch line)
#   organization  Error: unexpected answer from the K2 daemon (HTTP 200): <body>
#                 (_ORG_PY's hook, cli/src/permission.sh)
#   compute       {"error": "bad_response", "hint": "…"}  (the job reader
#                 after a submit, cli/src/compute-run.sh)
# and exit 1. tests/cli/ci_plain_errors.sh is ci's.
#
# How: a python stub daemon on a free loopback port answers {"ok": true}
# to every request. HOME is a temp dir, no real daemon, no network. Loud.
# Run with: bash tests/cli/unreadable_answer_plain.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-plain-answer-XXXXXX)"
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

DOC_HINT="k2 doc can't read the daemon's answer (a daemon on another K2 version?)"
DOC_ERR="Error: $DOC_HINT
{\"error\": {\"code\": \"bad_response\", \"hint\": \"$DOC_HINT\"}}"
ORG_ERR='Error: unexpected answer from the K2 daemon (HTTP 200): {"ok": true}'
CP_ERR='{"error": "bad_response", "hint": "k2 compute can'"'"'t read the daemon'"'"'s answer (a daemon on another K2 version?)"}'

# run <args...>: sets OUT (stdout), ERR (stderr), RC. stdin is empty.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# plain <want stderr tail> <args...>: exit 1, stderr ends with exactly
# that error (a line the family prints before it, a note, may come
# first), and no traceback anywhere.
plain() {
    local want="$1"
    shift
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 1 ] && ok "$label: exit 1" || bad "$label: exit $RC (want 1)"
    case "$ERR" in
        "$want"|*$'\n'"$want") ok "$label: the plain error" ;;
        *) bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2 ;;
    esac
    case "$OUT$ERR" in
        *Traceback*) bad "$label: a traceback" ;;
        *) ok "$label: no traceback" ;;
    esac
    [ -s "$REQ_LOG" ] && ok "$label: asked the daemon" || bad "$label: sent nothing"
}

echo "== doc: _doc_die's lines =="
for verb in status flush open leave lease lock unlock watch; do
    for tail in "x-k5-sweep" "-h"; do
        plain "$DOC_ERR" doc "$verb" -- "$tail"
    done
done

echo "== organization: the unexpected-answer line =="
for tail in "x-k5-sweep" "-h"; do
    plain "$ORG_ERR" organization bulletin set -- "$tail"
    plain "$ORG_ERR" organization delete -- "$tail"
done

echo "== compute: the job reader's bad_response =="
for tail in "x-k5-sweep" "-h"; do
    plain "$CP_ERR" compute run -- "$tail"
    plain "$CP_ERR" compute test -- "$tail"
done

echo ""
echo "unreadable_answer_plain: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
