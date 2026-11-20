#!/usr/bin/env bash
# k2 compute: the agent-feedback quick wins (0.46.17), against a tiny stub
# daemon on loopback (python3 http.server). No real daemon, no ~/.k2, no
# keychain: HOME is a throwaway dir.
#
# Covers: wait --timeout exits 124 with still_running and leaves the job
# alone (no cancel reaches the daemon), for one job, several, --any and
# --follow; --max-wait keeps exit 3; a failed job still exits 3; the two
# limits together are a usage error. `k2 compute list` is `jobs` (the same
# request, --json printed on stdout); `ls` stays `nodes`. `status` prints
# a running job's time from its start and the wait before it apart.

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

SANDBOX="$(mktemp -d /tmp/k2cfb-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]

def job(which):
    # "queued…" never ends; "failed…" ended failed; anything else done.
    short = which[:8]
    j = {"id": which, "short": short, "node": "linux-1", "argv": ["true"], "createdAt": 1000}
    if which.startswith("queued"):
        j.update({"state": "queued", "queue": {"position": 2, "of": 3, "reason": "parallel_full"}})
    elif which.startswith("failed"):
        j.update({"state": "failed", "reason": "spawn_failed", "startedAt": 1000, "endedAt": 1060})
    else:
        j.update({"state": "done", "exitCode": 0, "startedAt": 1000, "endedAt": 1060})
    return j

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
    def note(self, method, u, body=None):
        with open(LOG, "a") as f:
            f.write(json.dumps({"method": method, "path": u.path, "query": u.query, "body": body}) + "\n")
    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        self.note("GET", u)
        which = q.get("job", [""])[0]
        if u.path == "/cli/compute/jobs/get":
            # A long poll that answers soon: the CLI's deadline decides.
            time.sleep(0.2)
            return self.reply(200, job(which))
        if u.path == "/cli/compute/jobs":
            return self.reply(200, {"jobs": [dict(job("done0003"), workspace="ws", submitter="agent")]})
        if u.path == "/cli/compute/nodes" and q.get("detail") == ["1"]:
            return self.reply(200, {"nodes": [{"name": "mini-1", "os": "macos", "arch": "arm64", "status": {
                "state": "active", "parallelMax": 1,
                "running": [{"short": "run00001", "state": "running", "elapsedSecs": 120, "queuedSecs": 600, "workspace": "ws", "argv": "cargo test", "cpuMillis": 4000}],
                "queued": [{"short": "que00001", "state": "queued", "ageSecs": 300, "queuedSecs": 300, "position": 1, "workspace": "ws", "argv": "true", "pinned": False}]}}]})
        if u.path == "/cli/compute/nodes":
            return self.reply(200, {"nodes": []})
        if u.path == "/cli/compute/jobs/logs":
            time.sleep(0.2)
            j = job(which)
            return self.reply(200, {"cursor": 0, "chunks": [], "end": j["state"] != "queued", "job": j, "queue": j.get("queue"), "generation": 1})
        return self.reply(404, {"error": {"code": "not_found", "hint": self.path}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        u = urlparse(self.path)
        self.note("POST", u, body)
        return self.reply(404, {"error": {"code": "not_found", "hint": "stub: " + u.path}})

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
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

posts() { python3 -c 'import json,sys; print(sum(1 for l in open(sys.argv[1]) if json.loads(l)["method"] == "POST"))' "$LOG"; }

echo "== wait --timeout: exit 124, still_running, the job left alone =="
set +e
"$K2" compute wait queued01 --timeout 1s >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "a queued job past --timeout exits 124" "$rc" "124"
expect_has "it says still_running" "$(cat "$SANDBOX/err")" '"error": "still_running"'
expect_has "with the wait to run next" "$(cat "$SANDBOX/err")" '"hint": "k2 compute wait queued01"'
expect_eq "nothing was cancelled or changed (no POST)" "$(posts)" "0"
set +e
"$K2" compute wait done0001 queued02 --timeout 1s >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "several jobs, one still queued: 124" "$rc" "124"
set +e
"$K2" compute wait queued03 queued04 --any --timeout 1s >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "--any past --timeout: 124" "$rc" "124"
expect_has "--any says still_running" "$(cat "$SANDBOX/err")" '"error":"still_running"'
set +e
"$K2" compute wait queued05 --follow --timeout 1s >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "--follow past --timeout: 124" "$rc" "124"
expect_has "--follow says still_running" "$(cat "$SANDBOX/err")" '"error":"still_running"'
expect_eq "still nothing cancelled (no POST)" "$(posts)" "0"

echo "== --max-wait keeps exit 3; a failed job is 3 =="
set +e
"$K2" compute wait queued06 --max-wait 1s >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "--max-wait past: still 3" "$rc" "3"
expect_has "--max-wait says still_running" "$(cat "$SANDBOX/err")" '"error": "still_running"'
set +e
"$K2" compute wait queued07 --any --max-wait 1s >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "--any --max-wait past: still 3" "$rc" "3"
set +e
"$K2" compute wait failed01 --timeout 30s >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "a failed job exits 3 (not 124)" "$rc" "3"
expect_has "and names its end" "$(cat "$SANDBOX/err")" '"error": "failed"'
set +e
"$K2" compute wait done0002 --timeout 30s >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "a done job inside --timeout exits 0" "$rc" "0"

echo "== usage =="
set +e
"$K2" compute wait queued08 --timeout 1s --max-wait 1s >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "--timeout with --max-wait is a usage error" "$rc" "2"
set +e
"$K2" compute wait queued09 --timeout >/dev/null 2>"$SANDBOX/err"; rc=$?
set -e
expect_eq "--timeout with no duration is a usage error" "$rc" "2"
expect_has "the help says to use --timeout, not timeout(1)" "$("$K2" compute --help 2>&1)" "macOS has no timeout"
expect_has "the study page says the same" "$("$K2" study compute 2>&1)" "Use it, not timeout(1): macOS has no timeout command"

echo "== k2 compute list = jobs =="
last_get() { tail -1 "$LOG" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["method"], v["path"], v["query"])'; }
set +e
lout="$("$K2" compute list --mine --state done --json 2>"$SANDBOX/err")"; rc=$?
set -e
expect_eq "list exits 0" "$rc" "0"
expect_eq "list asks for the jobs, with its filters" "$(last_get)" "GET /cli/compute/jobs mine=1&state=done"
expect_has "list --json prints the jobs on stdout" "$lout" '"jobs"'
jout="$("$K2" compute jobs --mine --state done --json 2>/dev/null)"
expect_eq "list --json is jobs --json" "$lout" "$jout"
"$K2" compute ls --json >/dev/null 2>&1
expect_eq "ls is still nodes" "$(last_get | cut -d' ' -f1-2)" "GET /cli/compute/nodes"
expect_has "the help says list is jobs" "$("$K2" compute --help 2>&1)" "k2 compute list is the same; k2 compute ls is nodes"

echo "== status: running time from the start, the wait apart =="
sout="$("$K2" compute status 2>&1)"
expect_has "the running job shows its running time and the wait before it" "$sout" "running run00001  2m00s    ws           cargo test  (queued 10m00s first)"
expect_not "the wait is never shown as running time" "$sout" "running run00001  12m00s"

echo
echo "compute_agent_feedback_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
