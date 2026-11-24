#!/usr/bin/env bash
# k2 compute nodes / status / get: a node full of other servers' jobs and
# a queued auto job's "elsewhere" line, rendered against a tiny stub
# daemon on loopback (python3 http.server). No real daemon, no vault, no
# keychain, no ~/.k2: HOME is a throwaway dir.
#
# Covers: the node line counts the node's jobs for every server (runs
# 3/4, this server 0, other servers 3) and puts "busy (other servers)"
# first when only other servers' jobs fill it; the room is labelled "all
# servers"; this server's share line says it counts only this server's
# jobs; a node with room has no busy mark; status shows the same; get
# prints the move note naming the best other nodes in their own numbers;
# other servers are never named.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
# A here-string, not a pipe (pipefail and an early grep exit).
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2room-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

GB = 1 << 30

def node(name, free_cpu, cpus, running, room, share=None):
    n = {"id": name, "name": name, "state": "active", "online": True, "liveRoute": "direct",
         "platform": "ubuntu24.04-x86_64", "queued": 0, "running": 0, "nodeRunning": running,
         "mine": {"running": 0, "queued": 0}, "lastSeen": None, "room": room,
         "offer": {"control": "active", "available": True, "parallelMax": 4,
                   "caps": {"cpu_millis": cpus, "mem_bytes": 52 * GB},
                   "free": {"cpu_millis": free_cpu, "mem_bytes": 20 * GB, "disk_free_bytes": 300 * GB}}}
    if share:
        n["share"] = share
    status = {"state": "online/direct", "parallelMax": 4, "running": [], "queued": [],
              "free": {"cpuMillis": free_cpu, "memBytes": 20 * GB}, "caps": {"cpuMillis": cpus, "memBytes": 52 * GB}}
    return n, status

FULL_ROOM = {"running": 3, "parallelMax": 4, "thisServer": 0, "otherServers": 3, "full": True, "busy": "other_servers"}
FREE_ROOM = {"running": 0, "parallelMax": 4, "thisServer": 0, "otherServers": 0, "full": False, "busy": None}
SHARE = {"shared": True, "role": "tenant", "ownerLabel": "owner.example.test", "servers": 3, "state": "active",
         "line": "running 0/3 · queued 0/10 · today 0.0 of 8 CPU-h · max 2h/job · 4 CPUs · 8 GB"}

ELSEWHERE = "no other node has 8 free CPUs now: rig11 2.4/14.4 (other servers' jobs), rig12 2.4/14.4 (other servers' jobs), rig10 0/12 (other servers' jobs) unreserved"

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
            detail = "detail=1" in (u.query or "")
            rows = []
            for name, free_cpu, cpus, running, room, share in (("rig10", 0, 12000, 3, FULL_ROOM, SHARE), ("rig9", 12000, 12000, 0, FREE_ROOM, None)):
                n, st = node(name, free_cpu, cpus, running, room, share)
                if detail:
                    n["status"] = st
                rows.append(n)
            return self.reply(200, {"nodes": rows})
        if u.path == "/cli/compute/jobs/get":
            return self.reply(200, {
                "id": "abcdef0123456789", "short": "abcdef01", "node": "rig7", "state": "queued", "argv": ["true"],
                "placement": {"mode": "auto", "policy": "v2", "node": "rig7", "code": "queued_least_busy",
                              "why": "no node has 8 free CPUs now: rig7 4/12, rig10 0/12 (other servers' jobs) unreserved; least busy (2 running, 0 queued) · it moves if another frees up first",
                              "fits": "4 nodes fit"},
                "move": {"auto": True, "note": "K2 moves it when another node can start it now; " + ELSEWHERE, "elsewhere": ELSEWHERE},
                "wait": {"where": "server", "reason": "local_resource_limit", "words": "waiting for room: asks 8 CPUs; 4 of 12 CPUs unreserved",
                         "etaSecs": None, "etaBasis": "unknown", "elsewhere": ELSEWHERE},
            })
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

# Owner-style environment: port + token from a throwaway ~/.k2, no session.
export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"

echo "== nodes =="
out="$("$K2" compute nodes </dev/null)"
full="$(grep '^rig10 ' <<<"$out")"
free="$(grep '^rig9 ' <<<"$out")"
expect_has "a node full of other servers' jobs is marked first in its room" "$full" "busy (other servers) · all servers: cpu 0/12 unreserved"
expect_has "the run count is the node's, every server's" "$full" "runs 3/4 (this server 0, yours 0, other servers 3)"
expect_has "queued says whose" "$full" "queued 0 (this server, yours 0)"
expect_has "this server's share says it counts only this server's jobs" "$out" "your share (jobs of this server only): running 0/3"
expect_has "a node with room shows its room for all servers" "$free" "all servers: cpu 12/12 unreserved"
expect_not "a node with room has no busy mark" "$free" "busy"
expect_not "a node with room names no other servers' count" "$free" "other servers"
expect_not "the share's owner is not on the node line" "$full" "owner.example.test"

echo "== status =="
out="$("$K2" compute status </dev/null)"
full="$(grep '^rig10 ' <<<"$out")"
expect_has "status marks it too" "$full" "busy (other servers)"
expect_has "status counts every server's jobs" "$full" "runs 3/4 (this server 0, yours 0, other servers 3)"
expect_has "status labels the room" "$full" "all servers: cpu 0/12 unreserved"
expect_has "status's share line says whose" "$out" "your share (jobs of this server only): running 0/3"

echo "== get =="
out="$("$K2" compute get abcdef01 </dev/null)"
expect_has "the placement names the best candidates" "$out" "placed on rig7 (auto, v2): no node has 8 free CPUs now: rig7 4/12, rig10 0/12 (other servers' jobs) unreserved"
expect_has "the move note says why no other node fits" "$out" "move: K2 moves it when another node can start it now; no other node has 8 free CPUs now: rig11 2.4/14.4"
expect_has "the wait words stay" "$out" "waiting for room: asks 8 CPUs; 4 of 12 CPUs unreserved"

echo "== docs =="
study="$("$K2" study compute)"
expect_has "study explains the elsewhere line" "$study" "K2 checks every node every 5 seconds and moves the job to the first one"
expect_has "study explains busy (other servers)" "$study" "\"busy (other"

echo
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
