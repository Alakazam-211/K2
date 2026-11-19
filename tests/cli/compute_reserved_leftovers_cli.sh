#!/usr/bin/env bash
# k2 compute nodes / jobs / get / node health (0.46.16): which jobs hold a
# node's reservation, the leftover processes the node reports, and the
# start time of a job (the real start, not the submission, beside the
# queued time). Rendered against a tiny stub daemon on loopback (python3
# http.server). No real daemon, no vault, no keychain, no ~/.k2: HOME is
# a throwaway dir.
#
# Covers: a fully reserved node names this server's job holding it
# ("reserved by <short> (running, started 25m ago, 8 CPUs, 16.0 GB)") and
# counts other servers' jobs without naming them; a node with nothing
# reserved prints no such line; leftovers (count, pid, age, job) on the
# node line and in node health, said to be not killed; `jobs` shows
# "started Xm ago (queued Ym ago)" for a started job and "queued Ym ago"
# for one that hasn't started; `get` prints queued / started / ended and
# the job record's "killed N leftover process(es)" note.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
# A here-string, not a pipe (pipefail and an early grep exit).
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2reserved-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

GB = 1 << 30
NOW = int(time.time())

def node(name, free_cpu, room, reserved, leftovers):
    return {"id": name, "name": name, "state": "active", "online": True, "liveRoute": "direct",
            "platform": "macos15.5-arm64", "queued": 0, "running": len(reserved), "nodeRunning": room["running"],
            "lastSeen": NOW, "room": room, "reservedBy": reserved, "leftovers": leftovers,
            "offer": {"control": "active", "available": True, "parallelMax": 1,
                      "caps": {"cpu_millis": 8000, "mem_bytes": 16 * GB},
                      "free": {"cpu_millis": free_cpu, "mem_bytes": 0, "disk_free_bytes": 300 * GB}}}

FULL = {"running": 2, "parallelMax": 2, "thisServer": 1, "otherServers": 1, "full": True, "busy": "servers"}
IDLE = {"running": 0, "parallelMax": 2, "thisServer": 0, "otherServers": 0, "full": False, "busy": None}
HELD = [{"job": "beef0123456789ab", "short": "beef0123", "state": "running", "startedAt": NOW - 25 * 60,
         "createdAt": NOW - 31 * 60, "cpuMillis": 8000, "memBytes": 16 * GB}]
LEFT = {"count": 2, "at": NOW - 60, "procs": [
    {"pid": 4242, "age_secs": 27 * 3600, "cmd": "/var/k2node/cache/p/slots/0/target/release/k2-daemon --headless", "job_id": "dead0000aaaabbbb"},
    {"pid": 4243, "age_secs": 27 * 3600, "cmd": "sleep 3600"}]}

JOBS = [
    {"id": "beef0123456789ab", "short": "beef0123", "node": "k2n2-m1", "state": "running", "argv": ["cargo", "bench"],
     "createdAt": NOW - 31 * 60, "startedAt": NOW - 25 * 60},
    {"id": "cafe0123456789ab", "short": "cafe0123", "node": "k2n3-m1", "state": "queued", "argv": ["true"],
     "createdAt": NOW - 4 * 60, "startedAt": None},
]

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
    def do_GET(self):
        u = urlparse(self.path)
        if u.path == "/cli/compute/nodes":
            return self.reply(200, {"nodes": [node("k2n2-m1", 0, FULL, HELD, LEFT), node("k2n3-m1", 8000, IDLE, [], {"count": 0, "procs": [], "at": NOW})]})
        if u.path == "/cli/compute/jobs":
            return self.reply(200, {"jobs": JOBS})
        if u.path == "/cli/compute/jobs/get":
            return self.reply(200, {
                "id": "beef0123456789ab", "short": "beef0123", "node": "k2n2-m1", "state": "cancelled", "reason": "cancelled",
                "argv": ["cargo", "bench"], "createdAt": NOW - 31 * 60, "startedAt": NOW - 25 * 60, "endedAt": NOW - 60,
                "result": {"notes": ["killed 2 leftover process(es) the job left running: pid 4242 (TERM): k2-daemon --headless; pid 4243 (KILL): sleep 3600"]},
            })
        if u.path == "/cli/compute/nodes/health":
            return self.reply(200, {"node": "k2n2-m1", "online": True, "lastSeen": NOW, "nodeVersion": "0.46.16",
                                    "facts": {"leftovers": LEFT}})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

python3 "$SANDBOX/stub.py" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"

echo "== nodes =="
out="$("$K2" compute nodes </dev/null)"
expect_has "a fully reserved node names the job holding it" "$out" "reserved by beef0123 (running, started 25m ago, 8 CPUs, 16.0 GB), 1 job(s) of other servers"
expect_has "leftovers: the count, said not killed" "$out" "2 leftover process(es) of the job user with no live job (not killed"
expect_has "leftovers: pid, age and the job that left it" "$out" "pid 4242, 1d old, from job dead0000: /var/k2node/cache/p/slots/0/target/release/k2-daemon --headless"
expect_has "a leftover without a job id" "$out" "pid 4243, 1d old: sleep 3600"
idle="$(awk '/^k2n3-m1 /{f=1; print; next} /^[^ ]/{f=0} f' <<<"$out")"
expect_has "the idle node is listed" "$idle" "k2n3-m1"
expect_not "an idle node has no reserved line" "$idle" "reserved by"
expect_not "an idle node has no leftovers line" "$idle" "leftover"

echo "== jobs =="
out="$("$K2" compute jobs </dev/null)"
expect_has "a started job: from its real start, and its queued time" "$(grep '^beef0123' <<<"$out")" "started 25m ago (queued 31m ago)"
expect_has "a queued job: its queued time" "$(grep '^cafe0123' <<<"$out")" "queued 4m ago"
expect_not "a queued job never says started" "$(grep '^cafe0123' <<<"$out")" "started"

echo "== get =="
out="$("$K2" compute get beef0123 </dev/null)"
expect_has "get: queued, started and ended" "$out" "(31m ago) · started "
expect_has "get: the started time is the real start" "$out" "(25m ago) · ended "
expect_has "get: the job record says what was killed" "$out" "killed 2 leftover process(es) the job left running: pid 4242 (TERM)"

echo "== node health =="
out="$("$K2" compute node health k2n2-m1 </dev/null)"
expect_has "health: the leftover count" "$out" "leftovers     2 process(es) of the job user with no live job (not killed by the check)"
expect_has "health: each leftover" "$out" "leftover      pid 4242"

echo
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
