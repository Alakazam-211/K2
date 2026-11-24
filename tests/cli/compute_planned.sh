#!/usr/bin/env bash
# k2 compute: a held auto job's node is only where it would go now
# (`planned: true` in the job JSON), and a node's queued count names how
# many of them are such held jobs (`heldPlanned`). Against a tiny stub
# daemon on loopback (python3 http.server). No real daemon, no ~/.k2, no
# keychain: HOME is a throwaway dir.
#
# Covers:
#   get, held + planned      "held on this server (auto); planned: <node>"
#   get, queued + planned    the node says it is planned, not bound, and the
#                            placement line says "planned for", not "placed on"
#   get, not planned         unchanged ("job … on <node>: queued", "placed on")
#   jobs, planned            a held row: node "-", "would go to <node>"
#   nodes, heldPlanned > 0   "queued 13 (this server, yours 0; 11 held, planned here)"
#   nodes, heldPlanned 0 or absent   the queued part as before

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2planned-cli-XXXXXX)"
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

NOW = int(time.time())
PL = {"mode": "auto", "node": "rig12", "policy": "v2", "why": "soonest start"}
JOBS = {
    "h1": {"id": "h1000000", "short": "h1000000", "node": "rig12", "state": "queued", "planned": True,
           "argv": ["make"], "createdAt": NOW - 60, "attempt": 1, "generation": 1,
           "queue": {"held": True, "line": "held: waiting for a node with a free slot (fits: rig12)"},
           "wait": {"reason": "held"}, "placement": PL},
    "p1": {"id": "p1000000", "short": "p1000000", "node": "rig12", "state": "queued", "planned": True,
           "argv": ["make"], "createdAt": NOW - 60, "attempt": 1, "generation": 1, "placement": PL},
    "b1": {"id": "b1000000", "short": "b1000000", "node": "rig12", "state": "queued",
           "argv": ["make"], "createdAt": NOW - 60, "attempt": 1, "generation": 1, "placement": PL},
}
NODE = {"platform": "ubuntu24.04-x86_64", "state": "active", "online": True, "running": 2,
        "mine": {"running": 0, "queued": 0}, "lastSeen": NOW - 5}
NODES = {"nodes": [dict(NODE, name="rig12", queued=13, heldPlanned=11),
                   dict(NODE, name="rig15", queued=4, heldPlanned=0),
                   dict(NODE, name="rig17", queued=2)]}

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
    def do_GET(self):
        u = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(u.query).items()}
        if u.path == "/cli/compute/jobs/get" and q.get("job") in JOBS:
            return self.answer(200, JOBS[q["job"]])
        if u.path == "/cli/compute/jobs":
            return self.answer(200, {"jobs": [JOBS["p1"], JOBS["b1"]]})
        if u.path == "/cli/compute/nodes":
            return self.answer(200, NODES)
        return self.answer(404, {"error": {"code": "not_found", "hint": "stub"}})
    def do_POST(self):
        return self.answer(404, {"error": {"code": "not_found", "hint": "stub"}})

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

python3 "$SANDBOX/stub.py" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"
cd "$SANDBOX"

echo "== get: a held, planned job =="
out="$("$K2" compute get h1 2>&1)"
expect_has "held, with where it would go" "$out" "job h1000000: held on this server (auto); planned: rig12"
expect_has "the placement line says planned for" "$out" "  planned for rig12 (auto, v2): soonest start"
expect_not "never placed on" "$out" "placed on rig12"

echo "== get: a queued job whose node is only planned =="
out="$("$K2" compute get p1 2>&1)"
expect_has "the node is planned, not bound" "$out" "job p1000000 on rig12 (planned: held on this server): queued"
expect_has "planned for" "$out" "  planned for rig12 (auto, v2): soonest start"

echo "== get: a job bound to its node is unchanged =="
out="$("$K2" compute get b1 2>&1)"
expect_has "bound" "$out" "job b1000000 on rig12: queued"
expect_has "placed on" "$out" "  placed on rig12 (auto, v2): soonest start"

echo "== jobs: a planned job is a held row =="
out="$("$K2" compute jobs 2>&1)"
row="$(grep '^p1000000' <<<"$out")"
expect_has "held (auto)" "$row" "held (auto)"
expect_has "where it would go" "$row" "would go to rig12"
row="$(grep '^b1000000' <<<"$out")"
expect_has "a bound job keeps its node" "$row" "rig12"
expect_not "and no would-go" "$row" "would go to"

echo "== nodes: held jobs planned here =="
out="$("$K2" compute nodes 2>&1)"
expect_has "heldPlanned 11" "$(grep '^rig12' <<<"$out")" "queued 13 (this server, yours 0; 11 held, planned here)"
expect_has "heldPlanned 0: as before" "$(grep '^rig15' <<<"$out")" "queued 4 (this server, yours 0) "
expect_has "no heldPlanned (an older daemon): as before" "$(grep '^rig17' <<<"$out")" "queued 2 (this server, yours 0) "

echo "compute_planned: $pass passed, $fail failed"
[ "$fail" = 0 ]
