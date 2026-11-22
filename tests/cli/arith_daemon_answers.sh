#!/usr/bin/env bash
# Bash arithmetic never evaluates a daemon's answer that isn't digits. An
# answer can come from another server (a compute node someone else owns)
# or be changed on the way, and bash evaluates an array subscript inside
# $(( )) (it may hold a command substitution); a letter-only value names a
# shell variable, and bash evaluates that variable's value too.
#
# Answers covered:
#   store blob get       each chunk's len (a bad one is a plain error)
#   compute wait --follow  the job's startedAt, endedAt, signal and attempt
#   compute run (a split run, --shards on the daemon's side)  the signal
#
# And what the follower says when the generation it followed ended without
# ending the job: a move (the attempt number stays, no retry used) prints
# `moved: …`, never "infrastructure failure"; a retry names the end its
# attempt record has.
#
# A stub daemon on loopback answers with crafted values: a subscript with
# a command in it, and a name (ARITH_PROBE) of a variable this test puts in
# the CLI's environment holding one. The command must never run (a marker
# file), and valid answers still print and exit as before.
# No real daemon, no ~/.k2, no keychain: HOME is a throwaway dir.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if grep -qF -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (no '$3' in: $2)"; fi; }
expect_lacks() { if grep -qF -- "$3" <<<"$2"; then bad "$1 ('$3' in: $2)"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2arith-ans-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

# The stub answers each route from a JSON file the test writes:
#   blob.json   the /cli/zen/data/blob/get answer
#   logs.json   a list of /cli/compute/jobs/logs answers, one per call (the
#               last one repeats)
#   get.json    the /cli/compute/jobs/get answer
#   run.json    the /cli/compute/run answer
cat > "$SANDBOX/stub.py" <<'PY'
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

WORK = sys.argv[1]
FILES = {"/cli/zen/data/blob/get": "blob.json", "/cli/compute/jobs/logs": "logs.json",
         "/cli/compute/jobs/get": "get.json", "/cli/compute/run": "run.json"}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def answer(self):
        path = urlparse(self.path).path
        with open(os.path.join(WORK, "requests.log"), "a") as f:
            f.write("%s %s\n" % (self.command, path))
        name = FILES.get(path)
        p = os.path.join(WORK, name) if name else ""
        if not p or not os.path.exists(p):
            return self.reply(404, {"error": {"code": "not_found", "hint": "stub: " + path}})
        v = json.load(open(p))
        if path == "/cli/compute/jobs/logs":
            one = v[0]
            if len(v) > 1:
                json.dump(v[1:], open(p, "w"))
            v = one
        return self.reply(200, v)
    def do_GET(self):
        self.answer()
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(n)
        self.answer()

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(WORK, "port.tmp"), "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(os.path.join(WORK, "port.tmp"), os.path.join(WORK, "port"))
srv.serve_forever()
PY

python3 "$SANDBOX/stub.py" "$SANDBOX" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -s "$SANDBOX/port" ] && break; sleep 0.05; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
cat "$SANDBOX/port" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

MARK="$SANDBOX/mark"
# BASH_VERSINFO is always set, so set -u never stops the evaluation before
# the subscript (and its command) is expanded.
P="BASH_VERSINFO[\$(touch $MARK)]"
# What a letter-only answer names: a variable holding the same subscript.
export ARITH_PROBE="$P"

# run_k2 <label> <args…>: run k2 (stdout in OUT, stderr in ERR, exit in RC)
# and check the marker never appeared.
OUT="" ERR="" RC=0
run_k2() {
    local label="$1"
    shift
    rm -f "$MARK"
    RC=0
    "$K2" "$@" >"$SANDBOX/out" 2>"$SANDBOX/err" </dev/null || RC=$?
    OUT="$(cat "$SANDBOX/out")"
    ERR="$(cat "$SANDBOX/err")"
    if [ -e "$MARK" ]; then bad "$label: the command in the answer ran"; else ok "$label: the command in the answer never ran"; fi
}
J() { python3 -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1])))' "$1"; }
pyjson() { python3 -c "$1" "${@:2}"; }

echo "== store blob get: each chunk's len =="
blob() { # <len as JSON> <eof true|false>
    pyjson 'import base64, json, sys
print(json.dumps({"base64": base64.b64encode(b"hello").decode(), "eof": sys.argv[2] == "true",
                  "len": json.loads(sys.argv[1]), "offset": 0, "size": 5, "mime": "text/plain", "ref": "r1"}))' "$1" "$2" > "$SANDBOX/blob.json"
}
blob "$(J "\"$P\"")" false
rm -f "$SANDBOX/got"
run_k2 "blob len <subscript>" store blob get r1 --widget board --out "$SANDBOX/got"
expect_eq "blob len <subscript>: exit" "$RC" "1"
expect_has "blob len <subscript>: a plain error" "$ERR" "the daemon's answer wasn't readable"
[ ! -e "$SANDBOX/got" ] && ok "blob len <subscript>: no file written" || bad "blob len <subscript>: a file was written"
blob '"ARITH_PROBE"' false
run_k2 "blob len ARITH_PROBE (names a variable)" store blob get r1 --widget board --out "$SANDBOX/got"
expect_eq "blob len ARITH_PROBE: exit" "$RC" "1"
expect_has "blob len ARITH_PROBE: a plain error" "$ERR" "the daemon's answer wasn't readable"
blob '5' true
run_k2 "blob len 5" store blob get r1 --widget board --out "$SANDBOX/got"
expect_eq "blob len 5: exit" "$RC" "0"
expect_eq "blob len 5: the file" "$(cat "$SANDBOX/got" 2>/dev/null || echo missing)" "hello"
expect_eq "blob len 5: says what it got" "$OUT" "5 bytes, text/plain"

echo "== compute wait --follow: startedAt, endedAt, signal, attempt =="
# logs_end <job object as JSON> [more answers…]: the logs answers, in order.
logs() { pyjson 'import json, sys
print(json.dumps([{"chunks": [], "end": True, "cursor": 1, "generation": json.loads(a).pop("_gen", 1), "job": json.loads(a)} for a in sys.argv[1:]]))' "$@" > "$SANDBOX/logs.json"; }
done_job() { # <signal> <startedAt> <endedAt>, each as JSON
    printf '{"state": "done", "short": "job00001", "node": "n1", "exitCode": null, "signal": %s, "startedAt": %s, "endedAt": %s, "generation": 1}' "$1" "$2" "$3"
}
logs "$(done_job 9 1000 1125)"
run_k2 "signal 9" compute wait job00001 --follow
expect_eq "signal 9: exit 128+9" "$RC" "137"
expect_has "signal 9: says so with the time" "$ERR" "killed by signal 9 after 2m5s"
logs "$(done_job "$(J "\"$P\"")" 1000 1125)"
run_k2 "signal <subscript>" compute wait job00001 --follow
expect_eq "signal <subscript>: exit 128" "$RC" "128"
logs "$(done_job '"ARITH_PROBE"' 1000 1125)"
run_k2 "signal ARITH_PROBE (names a variable)" compute wait job00001 --follow
expect_eq "signal ARITH_PROBE: exit 128" "$RC" "128"
expect_has "signal ARITH_PROBE: no name printed" "$ERR" "killed by signal  after 2m5s"
logs "$(done_job 9 '"ARITH_PROBE"' 1125)"
run_k2 "startedAt ARITH_PROBE" compute wait job00001 --follow
expect_eq "startedAt ARITH_PROBE: exit 137" "$RC" "137"
expect_lacks "startedAt ARITH_PROBE: no time" "$ERR" " after "
logs "$(done_job 9 1000 "$(J "\"$P\"")")"
run_k2 "endedAt <subscript>" compute wait job00001 --follow
expect_eq "endedAt <subscript>: exit 137" "$RC" "137"
expect_lacks "endedAt <subscript>: no time" "$ERR" " after "
retry_job() { # <attempt as JSON>: attempt 1 failed, the daemon queued another
    printf '{"state": "queued", "short": "job00001", "node": "n2", "attempt": %s, "generation": 2, "ctl": {"failedAttempts": [{"reason": "node_lost"}]}}' "$1"
}
done0='{"state": "done", "short": "job00001", "node": "n2", "exitCode": 0, "generation": 2, "_gen": 2}'
logs "$(retry_job 2)" "$done0"
run_k2 "attempt 2" compute wait job00001 --follow
expect_eq "attempt 2: the retry's exit" "$RC" "0"
expect_has "attempt 2: names the failed attempt" "$ERR" "attempt 1 hit an infrastructure failure (node_lost); retrying as attempt 2 on n2"
logs "$(retry_job '"ARITH_PROBE"')" "$done0"
run_k2 "attempt ARITH_PROBE (names a variable)" compute wait job00001 --follow
expect_eq "attempt ARITH_PROBE: the retry's exit" "$RC" "0"
expect_has "attempt ARITH_PROBE: no number" "$ERR" "attempt ? hit an infrastructure failure"

echo "== compute wait --follow: a move is not a failure =="
# The generation changes and the attempt doesn't: a node gave the job back
# (or never answered a request to), and K2 placed it again. The daemon's
# attempt record says so ("moved"); the follower prints it as a move.
moved_logs() { # <attempt record as JSON>
    pyjson 'import json, sys
rec = json.loads(sys.argv[1])
moved = {"chunks": [], "end": True, "cursor": 1, "generation": 1, "attempts": [rec, {"attempt": 1, "generation": 2, "current": True}],
         "job": {"state": "queued", "short": "job00001", "node": "k2n10-vin", "attempt": 1, "generation": 2}}
done = {"chunks": [], "end": True, "cursor": 1, "generation": 2,
        "job": {"state": "done", "short": "job00001", "node": "k2n10-vin", "exitCode": 0, "generation": 2}}
print(json.dumps([moved, done]))' "$1" > "$SANDBOX/logs.json"
}
moved_logs '{"attempt": 1, "generation": 1, "current": false, "state": "assigned", "reason": "withdrawn", "nextCode": "withdrawn", "moved": "k2n9-vin gave it back from its queue before it started for k2n10-vin"}'
run_k2 "a move" compute wait job00001 --follow
expect_eq "a move: the job's exit" "$RC" "0"
expect_has "a move: says it moved" "$ERR" "[k2 compute] moved: k2n9-vin gave it back from its queue before it started for k2n10-vin; now on k2n10-vin (not a failure, no retry used)"
expect_lacks "a move: never an infrastructure failure" "$ERR" "infrastructure failure"
moved_logs '{"attempt": 1, "generation": 1, "current": false, "state": "assigned", "reason": "withdraw_unanswered", "asked": "withdraw_requested", "nextCode": "withdraw_unanswered", "moved": "k2n7-vin didn'"'"'t answer a request to give it back within 3 min; K2 took it back (k2n7-vin is told not to run it)"}'
run_k2 "an unanswered withdraw" compute wait job00001 --follow
expect_has "an unanswered withdraw: says it moved" "$ERR" "[k2 compute] moved: k2n7-vin didn't answer a request to give it back within 3 min; K2 took it back (k2n7-vin is told not to run it); now on k2n10-vin (not a failure, no retry used)"
expect_lacks "an unanswered withdraw: never an infrastructure failure" "$ERR" "infrastructure failure"
# A daemon from before the words: the attempt number stayed, so a move.
moved_logs '{"attempt": 1, "generation": 1, "current": false, "state": "assigned", "reason": "withdraw_requested", "next": "taken back from k2n9-vin'"'"'s queue before it started, to move it to k2n10-vin (no retry used)"}'
run_k2 "a move, older daemon" compute wait job00001 --follow
expect_has "a move, older daemon: says it moved" "$ERR" "[k2 compute] moved: taken back from k2n9-vin's queue before it started"
expect_lacks "a move, older daemon: never an infrastructure failure" "$ERR" "infrastructure failure"
# A real retry with its record: the record's own end, not "?".
pyjson 'import json
retried = {"chunks": [], "end": True, "cursor": 1, "generation": 1,
           "attempts": [{"attempt": 1, "generation": 1, "current": False, "state": "interrupted", "reason": "node_update", "nextCode": "retry_backoff", "moved": None},
                        {"attempt": 2, "generation": 2, "current": True}],
           "job": {"state": "queued", "short": "job00001", "node": "n2", "attempt": 2, "generation": 2}}
done = {"chunks": [], "end": True, "cursor": 1, "generation": 2, "job": {"state": "done", "short": "job00001", "node": "n2", "exitCode": 0, "generation": 2}}
print(json.dumps([retried, done]))' > "$SANDBOX/logs.json"
run_k2 "a retry with its record" compute wait job00001 --follow
expect_has "a retry with its record: the real end" "$ERR" "attempt 1 hit an infrastructure failure (node_update); retrying as attempt 2 on n2"
expect_lacks "a retry with its record: no move line" "$ERR" "moved:"

echo "== compute run, a split run: the signal =="
printf '%s' '{"job": {"id": "jobset01-0000", "short": "jobset01", "state": "queued"}, "shards": [{"index": 0, "node": "n1"}, {"index": 1, "node": "n2"}]}' > "$SANDBOX/run.json"
set_get() { # <signal as JSON>
    printf '{"state": "done", "short": "jobset01", "result": {"exit": null, "signal": %s, "durationSecs": 65, "shards": [{"index": 0, "state": "done", "exit": 0, "node": "n1"}, {"index": 1, "state": "done", "exit": null, "node": "n2"}]}}' "$1" > "$SANDBOX/get.json"
}
set_get 9
run_k2 "split run signal 9" compute run --no-source --mem 2G -- true
expect_eq "split run signal 9: exit 128+9" "$RC" "137"
set_get "$(J "\"$P\"")"
run_k2 "split run signal <subscript>" compute run --no-source --mem 2G -- true
expect_eq "split run signal <subscript>: exit 128" "$RC" "128"
set_get '"ARITH_PROBE"'
run_k2 "split run signal ARITH_PROBE (names a variable)" compute run --no-source --mem 2G -- true
expect_eq "split run signal ARITH_PROBE: exit 128" "$RC" "128"

echo
echo "arith_daemon_answers: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
