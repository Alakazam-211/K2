#!/usr/bin/env bash
# k2 compute: the exit code for a 404 comes from the verb, never from the
# caller's environment. Against a tiny stub daemon on loopback (python3
# http.server). No real daemon, no vault, no keychain, no ~/.k2: HOME is a
# throwaway dir.
#
# `k2 compute get <job>`, `status <job>`, `wait <job>...` and
# `wait --any <job>...` exit 4 when the job is not found; every other verb
# exits 3 on a 404. Before 0.46.18 the CLI read the not-found exit from
# _CP_404_EXIT, so an exported _CP_404_EXIT=0 made a missing job exit 0
# (and any 404 of any compute verb). Each case runs three times: with
# _CP_404_EXIT unset, exported as 0 and exported as 7; the exit must be the
# same every time.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
unset _CP_404_EXIT

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

SANDBOX="$(mktemp -d /tmp/k2cp404-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

# Every route answers 404: job_not_found for a job lookup, not_found for
# anything else. Each request is logged so the test can see it was asked.
cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

LOG = sys.argv[1]

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
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        with open(LOG, "a") as f:
            f.write(self.command + " " + u.path + "\n")
        if u.path == "/cli/compute/jobs/get":
            return self.reply(404, {"error": {"code": "job_not_found", "hint": "no such job"}})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})
    def do_GET(self):
        self.answer()
    def do_POST(self):
        self.answer()

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

LOG="$SANDBOX/requests.log"
: > "$LOG"
python3 "$SANDBOX/stub.py" "$LOG" "$SANDBOX/port" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

# Owner-style environment: port + token from a throwaway ~/.k2, no session.
export HOME="$SANDBOX/home"
mkdir -p "$HOME/.k2"
printf '%s' "$PORT" > "$HOME/.k2/heartbeat.port"
printf '%s' "owner-test-token" > "$HOME/.k2/heartbeat.token"

# check <label> <want exit> <want path> <k2 args...>: runs the verb with
# _CP_404_EXIT unset, exported as 0 and exported as 7.
check() {
    local label="$1" want="$2" path="$3" env_val err rc n_before
    shift 3
    for env_val in unset 0 7; do
        n_before="$(wc -l < "$LOG")"
        set +e
        if [ "$env_val" = unset ]; then
            err="$(env -u _CP_404_EXIT "$K2" "$@" 2>&1 </dev/null >/dev/null)"; rc=$?
        else
            err="$(_CP_404_EXIT="$env_val" "$K2" "$@" 2>&1 </dev/null >/dev/null)"; rc=$?
        fi
        set -e
        expect_eq "$label, _CP_404_EXIT $env_val: exit $want" "$rc" "$want"
        expect_has "$label, _CP_404_EXIT $env_val: asked $path" "$(tail -n +"$((n_before + 1))" "$LOG")" "$path"
        expect_has "$label, _CP_404_EXIT $env_val: prints the refusal" "$err" "not_found"
    done
}

echo "== verbs whose 404 means the job is not found: exit 4 =="
check "compute get <missing>" 4 "GET /cli/compute/jobs/get" compute get abc12345
check "compute status <missing>" 4 "GET /cli/compute/jobs/get" compute status abc12345
check "compute wait <missing>" 4 "GET /cli/compute/jobs/get" compute wait abc12345
check "compute wait <missing> <missing>" 4 "GET /cli/compute/jobs/get" compute wait abc12345 def67890
check "compute wait --any <missing> <missing>" 4 "GET /cli/compute/jobs/get" compute wait --any abc12345 def67890

echo "== other verbs keep exit 3 on a 404 =="
check "compute jobs" 3 "GET /cli/compute/jobs" compute jobs
check "compute cancel <missing>" 3 "POST /cli/compute/jobs/cancel" compute cancel abc12345

echo
echo "passed $pass, failed $fail"
[ "$fail" -eq 0 ]
