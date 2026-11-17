#!/usr/bin/env bash
# k2 compute logs/get/fetch and a job's earlier attempts: `logs` prints the
# latest attempt with a one-line header (on stderr) naming the earlier ones,
# `logs --attempt N` asks for attempt N, `get` lists every attempt with how
# it ended and why, `fetch --attempt N` asks for attempt N's files, and
# `k2 study compute` says which events retry or move a job on their own and
# how to turn retries off. A local fake daemon stands in and logs each
# request (path and query, no token). The daemon side is in
# crates/k2-daemon/tests/it_compute/compute_attempts_integration.rs.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; CLI="$CLI_UNDER_TEST"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-attempt-logs-XXXXXX)"
FAKE_PID=""
cleanup() {
    if [ -n "$FAKE_PID" ]; then kill "$FAKE_PID" 2>/dev/null || true; wait "$FAKE_PID" 2>/dev/null || true; fi
    rm -rf "$FAKE_DIR"
    hermetic_cli_cleanup
}
trap cleanup EXIT

cat >"$FAKE_DIR/fake.py" <<'PY'
import base64, http.server, json, sys, urllib.parse
log_path, port_path = sys.argv[1], sys.argv[2]
ATTEMPTS = [
    {"attempt": 1, "generation": 1, "node": "nodea-vin", "current": False, "startedAt": None, "endedAt": 1000,
     "state": "assigned", "reason": "node_queued", "why": "it never started", "next": "taken back from nodea-vin's queue before it started and sent again with the grant's new caps",
     "log": {"available": False, "bytes": 0}, "artifacts": {"status": "never_started", "files": []}},
    {"attempt": 1, "generation": 2, "node": "nodea-vin", "current": False, "startedAt": 1100, "endedAt": 9100,
     "state": "interrupted", "reason": "node_update", "why": "nodea-vin stopped it to update k2-node", "next": "retried automatically on another node",
     "log": {"available": True, "bytes": 2048}, "artifacts": {"status": "kept", "files": [{"name": "verdict.txt", "bytes": 42, "sha256": "ab" * 32}]}},
    {"attempt": 2, "generation": 3, "node": "nodeb-vin", "current": True, "startedAt": 9200, "endedAt": None,
     "state": "running", "reason": None, "why": None, "next": None,
     "log": {"available": True, "bytes": 12}, "artifacts": {"status": "pending", "files": []}},
]
JOB = {"id": "ab12cd34ef", "short": "ab12cd34", "node": "nodeb-vin", "state": "running", "argv": ["sh", "-c", "soak"],
       "attempt": 2, "generation": 3, "attempts": ATTEMPTS, "result": {"attempts": ATTEMPTS}}
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        u = urllib.parse.urlsplit(self.path)
        q = dict(urllib.parse.parse_qsl(u.query))
        with open(log_path, "a") as f:
            f.write("%s %s\n" % (u.path, json.dumps({k: v for k, v in q.items() if k != "token"}, sort_keys=True)))
        status, out = 200, {"ok": True}
        if u.path == "/cli/compute/jobs/get":
            out = JOB
        elif u.path == "/cli/compute/jobs/logs":
            att = q.get("attempt")
            if att not in (None, "1", "2"):
                status, out = 404, {"error": {"code": "attempt_not_found", "hint": "job ab12cd34 has attempts 1-2"}}
            else:
                text = b"verdict: suite one passed\n" if att == "1" else b"attempt two output\n"
                gen = 2 if att == "1" else 3
                chunks = [{"seq": 1, "stream": "out", "data": base64.b64encode(text).decode()}] if q.get("cursor") == "0" else []
                out = {"job": JOB, "attempt": int(att or 2), "generation": gen, "cursor": 50, "chunks": chunks, "end": True, "attempts": ATTEMPTS}
        elif u.path == "/cli/compute/jobs/artifact":
            out = {"job": JOB["id"], "short": "ab12cd34", "attempt": int(q.get("attempt") or 2), "artifacts": []}
        raw = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
open(port_path, "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$FAKE_DIR/fake.py" "$FAKE_DIR/requests.log" "$FAKE_DIR/port" &
FAKE_PID=$!
for _ in $(seq 1 50); do [ -s "$FAKE_DIR/port" ] && break; sleep 0.1; done
[ -s "$FAKE_DIR/port" ] || fail "fake daemon did not start"
export K2_PORT="$(cat "$FAKE_DIR/port")"
export K2_HOOK_TOKEN=owner-token

run() {
    set +e
    OUT="$("$CLI" "$@" 2>"$FAKE_DIR/stderr")"
    RC=$?
    ERR="$(cat "$FAKE_DIR/stderr")"
    set -e
}
has() { grep -qF -- "$2" <<<"$1" || fail "$3 (missing $(printf %q "$2") in: $1)"; }
lacks() { if grep -qF -- "$2" <<<"$1"; then fail "$3 (has $(printf %q "$2"))"; fi; }

echo "== logs: the latest attempt, with a header naming the earlier one =="
: >"$FAKE_DIR/requests.log"
run compute logs ab12cd34
[ "$RC" = "0" ] || fail "logs exits 0 (got $RC: $ERR)"
has "$OUT" "attempt two output" "logs prints the latest attempt"
lacks "$OUT" "suite one passed" "logs without --attempt prints only the latest attempt"
has "$ERR" "attempt 2 of 2" "the header names the attempt shown"
has "$ERR" "attempt 1 on nodea-vin" "the header names the earlier attempt and its node"
has "$ERR" "--attempt 1" "the header says how to read it"
lacks "$OUT" "attempt 2 of 2" "the header is on stderr, so a pipe reads only the log"

echo "== logs --attempt 1 =="
: >"$FAKE_DIR/requests.log"
run compute logs ab12cd34 --attempt 1
[ "$RC" = "0" ] || fail "logs --attempt 1 exits 0 (got $RC: $ERR)"
has "$OUT" "verdict: suite one passed" "attempt 1's log"
has "$(grep /cli/compute/jobs/logs "$FAKE_DIR/requests.log")" '"attempt": "1"' "the CLI asks for attempt 1"
run compute logs ab12cd34 --attempt 9
[ "$RC" = "3" ] || fail "an attempt the job never had exits 3 (got $RC)"
has "$ERR" "attempt_not_found" "and says so"
run compute logs ab12cd34 --attempt x
[ "$RC" = "2" ] || fail "--attempt x is a usage error (got $RC)"
has "$ERR" "--attempt N" "the usage line names --attempt N"

echo "== logs --json =="
run compute logs ab12cd34 --attempt 1 --json
[ "$RC" = "0" ] || fail "logs --json exits 0 (got $RC: $ERR)"
python3 -c '
import json, sys
v = json.loads(sys.argv[1])
assert v["attempt"] == 1 and v["generation"] == 2, v
assert v["log"] == "verdict: suite one passed\n", v
assert [a["generation"] for a in v["attempts"]] == [1, 2, 3], v
assert v["attempts"][1]["log"]["available"] is True, v
' "$OUT" || fail "logs --json: attempt, log text and the attempts list"

echo "== get: every attempt and why it ended =="
run compute get ab12cd34
[ "$RC" = "0" ] || fail "get exits 0 (got $RC: $ERR)"
has "$OUT" "attempts (3):" "get lists the attempts"
has "$OUT" "attempt 1 · generation 2 · nodea-vin" "the attempt that ran"
has "$OUT" "interrupted/node_update: nodea-vin stopped it to update k2-node" "how it ended, in plain words"
has "$OUT" "retried automatically on another node" "what K2 did next"
has "$OUT" "k2 compute logs ab12cd34 --attempt 1" "how to read its log"
has "$OUT" "k2 compute fetch ab12cd34 --attempt 1" "how to fetch its files"
has "$OUT" "never started" "a generation that never started says so"
run compute get ab12cd34 --json
python3 -c '
import json, sys
v = json.loads(sys.argv[1])
assert len(v["attempts"]) == 3 and v["attempts"][1]["reason"] == "node_update", v
' "$OUT" || fail "get --json carries the attempts"

echo "== fetch --attempt 1 =="
: >"$FAKE_DIR/requests.log"
run compute fetch ab12cd34 --attempt 1 --out "$FAKE_DIR/out"
[ "$RC" = "0" ] || fail "fetch --attempt 1 exits 0 (got $RC: $ERR)"
has "$(cat "$FAKE_DIR/requests.log")" '"attempt": "1"' "the CLI asks for attempt 1's files"
run compute fetch ab12cd34 --attempt 0
[ "$RC" = "2" ] || fail "--attempt 0 is a usage error (got $RC)"

echo "== help and study =="
help="$("$CLI" compute --help)"
has "$help" "k2 compute logs <job> [--attempt N]" "help: logs --attempt"
has "$help" "[--attempt N]" "help: fetch --attempt"
study="$("$CLI" study compute)"
has "$study" "AUTOMATIC RETRIES AND MOVES" "study: the section"
has "$study" "node_update" "study: a node update retries"
has "$study" "--retries 0" "study: how to turn retries off"
has "$study" "k2 compute logs <job> --attempt 1" "study: earlier attempts' logs"

echo "compute_attempt_logs_cli: ok"
