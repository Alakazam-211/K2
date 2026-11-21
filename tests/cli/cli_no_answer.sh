#!/usr/bin/env bash
# A daemon that never answers gets a plain error naming the route and how
# long the CLI waited, and a non-zero exit; never the downstream reader's
# JSON parse error.
#
# Field case: one workspace's `k2 heartbeat session <name>` and `k2 inbox`
# hung, then printed `Error: Expecting value: line 1 column 1 (char 0)`
# (python's json.loads on an empty body). `k2 inbox` could even say
# "No items." for a request nobody answered.
#
# A python stub daemon answers /cli/heartbeat/list and /cli/inbox/list per a
# mode file. Cases (each through the real cli/k2, no live K2):
#   1. hangs past the CLI's 30 s limit → "didn't answer … within N s", fails,
#      and the request reached the stub once (a GET, then a POST: k2 done)
#   2. closes the connection with no reply → "gave an empty answer", fails
#      (heartbeat session, and inbox list, which used to say "No items.")
#   3. a 500 with an empty body → the same plain error
#   4. a cell socket that closes with no reply → quiet; the TCP try answers
#   5. a normal answer is untouched
# Case 1 waits out the 30 s limit twice. Exit status: 1, or 141 when the CLI
# ended a reader beside the request (see _k2_end_command in cli/k2).
# Run with: bash tests/cli/cli_no_answer.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

[ -x "$K2" ] || { echo "FAIL: $K2 not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))" ;; esac; }
assert_absent() { case "$2" in *"$3"*) bad "$1 (unexpected $(printf %q "$3") in $(printf %q "$2"))" ;; *) ok "$1" ;; esac; }
# The command failed, with our one line as its only stderr. The status is 1,
# or 141 when the CLI had to end a reader beside the request (SIGPIPE).
assert_failed() {
    case "$rc" in 1|141) ok "${1}exit $rc" ;; *) bad "${1}exit $rc (want 1 or 141)" ;; esac
    assert_eq "${1}one line on stderr" "$(printf '%s\n' "$err" | grep -c .)" "1"
    assert_absent "${1}no job report" "$err" "Terminated"
}

# Short base: a Unix socket path is capped near 104 bytes on macOS.
WORKDIR="$(mktemp -d /tmp/k2na-XXXXXX)"
PIDS=()
cleanup() {
    local p
    for p in "${PIDS[@]:-}"; do
        [ -n "$p" ] && kill "$p" 2>/dev/null || true
    done
    rm -rf "$WORKDIR"
}
trap cleanup EXIT
mkdir -p "$WORKDIR/home/.k2" "$WORKDIR/project"

# The stub daemon. $WORKDIR/mode: ok | hang | drop | empty500.
python3 - "$WORKDIR" <<'PY' &
import json, os, socketserver, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse
root = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        return
    def _go(self):
        n = int(self.headers.get("Content-Length", 0) or 0)
        if n:
            self.rfile.read(n)
        path = urlparse(self.path).path
        with open(os.path.join(root, "requests.log"), "a") as f:
            f.write(path + "\n")
        mode = open(os.path.join(root, "mode")).read().strip()
        if mode == "hang":
            time.sleep(45)
            return
        if mode == "drop":
            self.close_connection = True
            return
        if mode == "empty500":
            self.send_response(500)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if path == "/cli/heartbeat/list":
            body = json.dumps([{"name": "nightly", "useWorkspaceSession": True}])
        elif path == "/cli/inbox/list":
            body = "[]"
        else:
            body = json.dumps({"ok": False, "error": "stub " + path})
        data = body.encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self): self._go()
    def do_POST(self): self._go()
class S(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True
httpd = S(("127.0.0.1", 0), H)
open(os.path.join(root, "stub.port"), "w").write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
PIDS+=($!)
disown "$!" 2>/dev/null || true

# A cell socket that accepts, reads the request and closes with no reply.
python3 - "$WORKDIR/c.sock" "$WORKDIR" <<'PY' &
import os, socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind(sys.argv[1])
s.listen(8)
open(os.path.join(sys.argv[2], "cell.ready"), "w").write("1")
while True:
    c, _ = s.accept()
    try:
        c.recv(65536)
        with open(os.path.join(sys.argv[2], "cell.log"), "a") as f:
            f.write("hit\n")
    finally:
        c.close()
PY
PIDS+=($!)
disown "$!" 2>/dev/null || true

for _ in $(seq 1 100); do
    [ -f "$WORKDIR/stub.port" ] && [ -f "$WORKDIR/cell.ready" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/stub.port" ] || { echo "FAIL: stub daemon never bound" >&2; exit 1; }
[ -S "$WORKDIR/c.sock" ] || { echo "FAIL: cell socket never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORKDIR/stub.port")"
echo "owner-no-answer-token" >"$WORKDIR/home/.k2/heartbeat.token"
chmod 600 "$WORKDIR/home/.k2/heartbeat.token"

# Owner outside K2: TCP only.
run_tcp() {
    env -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOST -u K2_CELL -u K2_API_CELL -u K2_SECRETS_DIR \
        -u K2_HOOK_TOKEN -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK -u K2_REMOTE_TOKEN -u K2_AIRGAP \
        -u CODEX_THREAD_ID -u CODEX_HOME -u CODEX_SANDBOX \
        HOME="$WORKDIR/home" K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WORKDIR/project" \
        "$K2" "$@"
}
# A session in a K2 terminal: passport + cell socket, TCP as the fallback.
run_cell() {
    env -u K2SO_HOOK_TOKEN -u K2SO_PORT -u K2_HOST -u K2_CELL -u K2_API_CELL -u K2_SECRETS_DIR \
        -u K2_REMOTE_TOKEN -u K2_AIRGAP -u CODEX_THREAD_ID -u CODEX_HOME -u CODEX_SANDBOX \
        HOME="$WORKDIR/home" K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WORKDIR/project" \
        K2_HOOK_TOKEN="test-session.passport" K2_HOOK_SOCK="$WORKDIR/c.sock" K2SO_HOOK_SOCK="$WORKDIR/c.sock" \
        "$K2" "$@"
}
# run_case NAME CMD… → $rc, $out, $err, $took
run_case() {
    local name="$1"; shift
    local t0 t1
    t0=$(date +%s)
    set +e
    out="$("$@" 2>"$WORKDIR/$name.err")"
    rc=$?
    set -e
    t1=$(date +%s)
    took=$((t1 - t0))
    err="$(cat "$WORKDIR/$name.err")"
}
echo "== 1. the daemon hangs past the CLI's limit =="
echo hang >"$WORKDIR/mode"
: >"$WORKDIR/requests.log"
run_case hang run_tcp heartbeat session nightly
# It reached the daemon, so it is never sent again (newer curl words its
# -v trace differently; the CLI must not read that as a connect timeout).
assert_eq "the request reached the daemon exactly once" "$(grep -c . "$WORKDIR/requests.log")" "1"
assert_failed ""
assert_eq "nothing on stdout" "$out" ""
if [[ "$err" =~ "Error: K2's server didn't answer /cli/heartbeat/list within "3[0-9]" s; run k2 daemon status" ]]; then
    ok "names the route and the wait: $err"
else
    bad "plain timeout line missing: $(printf %q "$err")"
fi
assert_absent "no JSON parse error" "$err" "Expecting value"
if [ "$took" -le 40 ]; then ok "ended within the limit (${took} s)"; else bad "took ${took} s"; fi

echo "== 1b. a POST the daemon never answers is sent once =="
echo hang >"$WORKDIR/mode"
: >"$WORKDIR/requests.log"
run_case hang-done run_tcp done --agent nightly
assert_failed "done: "
if [[ "$err" =~ "Error: K2's server didn't answer /cli/done within "3[0-9]" s; run k2 daemon status" ]]; then
    ok "done: names the route and the wait"
else
    bad "done: plain timeout line missing: $(printf %q "$err")"
fi
assert_eq "done: the POST reached the daemon exactly once" "$(grep -c . "$WORKDIR/requests.log")" "1"
if [ "$took" -le 40 ]; then ok "done: ended within the limit (${took} s)"; else bad "done: took ${took} s"; fi

echo "== 2. the connection closes with no reply =="
echo drop >"$WORKDIR/mode"
run_case drop-hb run_tcp heartbeat session nightly
assert_failed "heartbeat session: "
assert_contains "heartbeat session: plain line" "$err" "Error: K2's server gave an empty answer to /cli/heartbeat/list after "
assert_contains "heartbeat session: next step" "$err" "; run k2 daemon status"
assert_absent "heartbeat session: no JSON parse error" "$err" "Expecting value"
run_case drop-inbox run_tcp inbox
assert_failed "inbox: "
assert_absent "inbox: never 'No items.' for a request nobody answered" "$out" "No items."
assert_contains "inbox: plain line" "$err" "Error: K2's server gave an empty answer to /cli/inbox/list after "
# The reader was stopped before it read the empty body: had it run, this
# capture would hold its line (it still held the stdout pipe).
assert_absent "inbox: no parse error" "$err" "failed to parse"

echo "== 3. a 500 with an empty body =="
echo empty500 >"$WORKDIR/mode"
run_case e500 run_tcp heartbeat session nightly
assert_failed ""
assert_contains "plain line" "$err" "Error: K2's server gave an empty answer to /cli/heartbeat/list after "
assert_absent "no JSON parse error" "$err" "Expecting value"

echo "== 4. the cell socket closes with no reply; TCP answers =="
echo ok >"$WORKDIR/mode"
: >"$WORKDIR/requests.log"
run_case cell run_cell heartbeat session nightly
assert_eq "exit 0" "$rc" "0"
assert_contains "the answer prints" "$out" "pinned"
assert_absent "the cell try stays quiet" "$err" "K2's server"
assert_contains "the cell socket was tried" "$(cat "$WORKDIR/cell.log" 2>/dev/null)" "hit"
assert_contains "then TCP answered" "$(cat "$WORKDIR/requests.log")" "/cli/heartbeat/list"

echo "== 5. a normal answer is untouched =="
run_case normal run_tcp heartbeat session nightly
assert_eq "exit 0" "$rc" "0"
assert_contains "the answer prints" "$out" "pinned"
assert_eq "nothing on stderr" "$err" ""

echo ""
echo "cli_no_answer: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
