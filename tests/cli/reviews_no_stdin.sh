#!/usr/bin/env bash
# `k2 reviews` never reads the caller's stdin. When K2's answer can't be
# read it prints one error line on stderr and exits 1.
#
# Regression (0.46.17): the reader ended with `|| cat`, so an answer it
# couldn't read made `cat` copy the caller's own stdin to stdout (and wait
# on a terminal). Now: a list prints its rows, an empty list "No pending
# reviews.", and an answer that isn't JSON, an empty answer or an error
# object is an error line on stderr with exit 1. stdin carries a marker in
# every case and must never show up.
#
# How: a python stub daemon on a free loopback port; the answer to
# /cli/reviews comes from a file the test writes. HOME is a temp dir, no
# real daemon, no network. Loud.
# Run with: bash tests/cli/reviews_no_stdin.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-reviews-stdin-XXXXXX)"
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
        body_file = work + "/answer"
        data = open(body_file, "rb").read() if path == "/cli/reviews" else b'{"ok": true}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
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

MARK="STDIN-MARKER-must-not-print"
# reviews_with <answer>: sets OUT, ERR, RC with the marker on stdin.
reviews_with() {
    printf '%s' "$1" >"$WORK/answer"
    set +e
    OUT="$(printf '%s\n' "$MARK" | "$K2" reviews 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
    case "$OUT$ERR" in *"$MARK"*) bad "$2: stdin was copied out" ;; *) ok "$2: stdin not read" ;; esac
}
reviews_with '[{"agentName": "ada", "branch": "feat/x", "workItems": [1], "diffSummary": [1, 2]}]' "a list"
[ "$RC" -eq 0 ] && ok "a list: exit 0" || bad "a list: exit $RC"
case "$OUT" in *"ada"*"feat/x"*"1 items, 2 files changed"*) ok "a list: the row" ;; *) bad "a list: $OUT" ;; esac
reviews_with '[]' "an empty list"
[ "$RC" -eq 0 ] && [ "$OUT" = "No pending reviews." ] && ok "an empty list: No pending reviews." || bad "an empty list: rc $RC $OUT"
reviews_with 'not json at all' "not JSON"
[ "$RC" -eq 1 ] && ok "not JSON: exit 1" || bad "not JSON: exit $RC"
[ -z "$OUT" ] && ok "not JSON: nothing on stdout" || bad "not JSON: stdout $OUT"
case "$ERR" in "Error: k2 reviews: K2 answered /cli/reviews with something that is not JSON") ok "not JSON: one error line" ;; *) bad "not JSON: stderr $ERR" ;; esac
reviews_with '' "no answer"
[ "$RC" -eq 1 ] && ok "no answer: exit 1" || bad "no answer: exit $RC"
case "$ERR" in *"Error: k2 reviews: no answer from K2 on /cli/reviews"*) ok "no answer: says so" ;; *) bad "no answer: stderr $ERR" ;; esac
reviews_with '{"error": {"code": "forbidden", "hint": "not yours"}}' "an error object"
[ "$RC" -eq 1 ] && ok "an error object: exit 1" || bad "an error object: exit $RC"
case "$ERR" in *"Error: k2 reviews: not yours"*) ok "an error object: its hint" ;; *) bad "an error object: stderr $ERR" ;; esac

echo ""
echo "reviews_no_stdin: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
