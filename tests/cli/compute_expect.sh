#!/usr/bin/env bash
# k2 compute run / test --expect <duration>|unknown: what reaches the
# daemon, and what get and wait print of the daemon's expected / took /
# estSecs, against a tiny stub daemon on loopback (python3 http.server).
# No real daemon, no ~/.k2, no keychain: HOME is a throwaway dir.
#
# Covers:
#   run --expect 12m           expectSecs 720, no expectUnknown
#   run --expect 1h30m         expectSecs 5400
#   run --expect unknown       expectUnknown true, no expectSecs
#   the last --expect wins     never both keys
#   no --expect                neither key
#   --expect soon / 1.5h / a missing value: exit 2 with the one usage
#                              line, nothing sent
#   test --expect 12m          passes it through to the run
#   get, a queued job          "expected: <words>" and the ahead list's ~Nm
#   get, a done job            its took line, no expected line
#   wait, a done job           its took line after the job's end line

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2expect-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]
NOW = int(time.time())
EXPECTED = {"secs": 1500, "basis": "history", "runs": 8, "said": 120, "historySecs": 1500,
            "expectIgnoredUntil": None,
            "words": "you said 2m00s; its last 8 earlier runs took ~25m: queued as 25m"}
JOBS = {
    "q1": {"id": "q1000000", "short": "q1000000", "node": "linux-1", "state": "queued",
           "argv": ["make", "test"], "createdAt": NOW - 60, "attempt": 1, "generation": 1,
           "expected": EXPECTED,
           "queue": {"position": 2, "of": 3, "reason": "busy", "message": "waiting",
                     "line": "queued on linux-1: 2 of 3 · expected you said 2m00s",
                     "expected": EXPECTED, "order": "shortest expected first",
                     "ahead": [{"short": "a1", "state": "queued", "workspace": "/w", "ageSecs": 30,
                                "argv": "cargo test", "estSecs": 610},
                               {"short": "a2", "state": "running", "workspace": "/w", "elapsedSecs": 61,
                                "argv": "make"}]}},
    "d1": {"id": "d1000000", "short": "d1000000", "node": "linux-1", "state": "done", "exitCode": 0,
           "argv": ["make", "test"], "createdAt": NOW - 800, "startedAt": NOW - 760, "endedAt": NOW - 60,
           "attempt": 1, "generation": 1, "expected": EXPECTED,
           "took": "took 11m40s; next time: --expect 12m", "nextExpect": "--expect 12m"},
}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def answer(self, code, v):
        b = json.dumps(v).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def record(self, entry):
        with open(LOG, "a") as f:
            f.write(json.dumps(entry, sort_keys=True) + "\n")
    def do_GET(self):
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        if u.path == "/cli/compute/jobs/get" and q.get("job") in JOBS:
            return self.answer(200, JOBS[q["job"]])
        self.record({"method": "GET", "path": u.path, "query": q})
        return self.answer(404, {"error": {"code": "not_found", "hint": "stub"}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        body.pop("clientJobId", None)
        self.record({"method": "POST", "path": urlparse(self.path).path, "body": body})
        return self.answer(404, {"error": {"code": "no_node_fits", "hint": "stub: nothing runs here"}})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.jsonl"
: > "$LOG"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

# k2_sent <args…>: run k2 (the stub refuses) and print the one request it
# sent, as sorted JSON. Exactly one request, or a FAIL.
k2_sent() {
    local before after
    before="$(wc -l < "$LOG")"
    "$K2" "$@" >/dev/null 2>&1 || true
    after="$(wc -l < "$LOG")"
    if [ "$((after - before))" != "1" ]; then
        bad "k2 $* sent $((after - before)) requests, want 1"
        echo "{}"
        return 0
    fi
    tail -1 "$LOG"
}
# field <json> <python expression on v>
field() { python3 -c 'import json,sys; v=json.loads(sys.argv[1]); print(json.dumps(eval(sys.argv[2])))' "$1" "$2"; }
ex() { field "$1" '[v["body"].get("expectSecs"), v["body"].get("expectUnknown"), "expectSecs" in v["body"] and "expectUnknown" in v["body"]]'; }

echo "== run --expect: what the body carries =="
expect_eq "--expect 12m" "$(ex "$(k2_sent compute run --no-source --mem 2G --expect 12m -- true)")" '[720, null, false]'
expect_eq "--expect 1h30m" "$(ex "$(k2_sent compute run --no-source --mem 2G --expect 1h30m -- true)")" '[5400, null, false]'
expect_eq "--expect 90 (plain seconds)" "$(ex "$(k2_sent compute run --no-source --mem 2G --expect 90 -- true)")" '[90, null, false]'
expect_eq "--expect unknown" "$(ex "$(k2_sent compute run --no-source --mem 2G --expect unknown -- true)")" '[null, true, false]'
expect_eq "unknown, then 5m: the last wins" "$(ex "$(k2_sent compute run --no-source --mem 2G --expect unknown --expect 5m -- true)")" '[300, null, false]'
expect_eq "5m, then unknown: the last wins" "$(ex "$(k2_sent compute run --no-source --mem 2G --expect 5m --expect unknown -- true)")" '[null, true, false]'
expect_eq "no --expect: neither key" "$(ex "$(k2_sent compute run --no-source --mem 2G -- true)")" '[null, null, false]'

echo "== run --expect: a bad or missing value is the usage line, nothing sent =="
for bad_args in "soon" "1.5h" "12 minutes" ""; do
    before="$(wc -l < "$LOG")"
    set +e
    if [ -n "$bad_args" ]; then
        "$K2" compute run --no-source --mem 2G --expect "$bad_args" -- true >/dev/null 2>"$SANDBOX/err"
    else
        "$K2" compute run --no-source --mem 2G --expect >/dev/null 2>"$SANDBOX/err"
    fi
    rc=$?
    set -e
    expect_eq "--expect '$bad_args' exits 2" "$rc" "2"
    expect_eq "... with the usage line" "$(cat "$SANDBOX/err")" "usage: --expect <duration>|unknown (e.g. --expect 12m; unknown only on a first run)"
    expect_eq "... and sends nothing" "$(wc -l < "$LOG")" "$before"
done

echo "== test --expect passes it to the run =="
expect_eq "test --expect 12m" "$(ex "$(k2_sent compute test --no-source --mem 2G --expect 12m -- -p x)")" '[720, null, false]'
expect_eq "test --expect unknown" "$(ex "$(k2_sent compute test --no-source --mem 2G --expect unknown -- -p x)")" '[null, true, false]'

echo "== get: a queued job says what K2 expects =="
out="$("$K2" compute get q1 2>&1)"
expect_has "the expected line" "$out" "  expected: you said 2m00s; its last 8 earlier runs took ~25m: queued as 25m"
expect_has "the queue line as the daemon wrote it" "$out" "  queued on linux-1: 2 of 3 · expected you said 2m00s"
expect_has "an ahead job with estSecs: ~Nm" "$out" "  ahead: a1        queued    /w             0m30s ~11m  cargo test"
expect_has "an ahead job without: nothing more" "$out" "  ahead: a2        running   /w             1m01s  make"
expect_not "no took line while queued" "$out" "next time:"

echo "== get: a done job prints what it took =="
out="$("$K2" compute get d1 2>&1)"
expect_has "the took line" "$out" "  took 11m40s; next time: --expect 12m"
expect_not "no expected line once done" "$out" "expected:"

echo "== wait: a done job prints what it took =="
set +e
out="$("$K2" compute wait d1 2>&1)"
rc=$?
set -e
expect_eq "wait exits with the job's code" "$rc" "0"
expect_has "the end line" "$out" "job d1000000 on linux-1: exit 0 after 11m40s"
expect_has "then the took line" "$out" "  took 11m40s; next time: --expect 12m"
set +e
out="$("$K2" compute wait d1 --any 2>&1)"
rc=$?
set -e
expect_eq "wait --any exits with the job's code" "$rc" "0"
expect_has "wait --any: the took line" "$out" "  took 11m40s; next time: --expect 12m"

echo "== help and schema tell agents to give it =="
help="$("$K2" compute --help 2>&1)"
expect_has "compute --help names --expect" "$help" "--expect 12m|unknown  always pass it too"
expect_has "compute --help: test takes it" "$help" "k2 compute test [<node>] [--shards N] [--expect 12m|unknown]"

echo "compute_expect: $pass passed, $fail failed"
[ "$fail" = 0 ]
