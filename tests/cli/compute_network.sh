#!/usr/bin/env bash
# k2 compute run --network none / --netns-dummy (0.46.14): CLI shapes
# against a tiny stub daemon on loopback (python3 http.server). No real
# daemon, no ~/.k2, no keychain: HOME is a throwaway dir.
#
# Covers: `--network none` sends network: "none"; `--network on|no-lan|bogus`
# exit 2 with no request (a job only narrows); `--netns-dummy` (repeatable)
# sends netnsDummies as given and network "none" (it implies it); a
# malformed one, or five, exit 2 with no request; no flag sends neither
# key; `compute test` passes both through; job_network_needs_update exits
# 3 with the daemon's hint; `get` shows the network the job asked for and
# `get --json` carries result.network; help, study and --schema name them.

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

SANDBOX="$(mktemp -d /tmp/k2net-cli-XXXXXX)"
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

LOG = sys.argv[1]
NET = {"mode": "none", "dummies": [{"name": "sewlane0", "addr": "10.254.254.1/32"}], "line": "none; dummies: sewlane0 10.254.254.1/32"}

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
        if u.path == "/cli/compute/jobs/get":
            return self.reply(200, {"id": "abcd1234" * 4, "short": "abcd1234", "node": "k2n7", "state": "done", "exitCode": 0, "argv": ["true"],
                                    "network": NET, "result": {"network": NET}})
        return self.reply(404, {"error": {"code": "not_found", "hint": self.path}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        u = urlparse(self.path)
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "body": body}) + "\n")
        if u.path == "/cli/compute/run":
            return self.reply(409, {"error": {"code": "job_network_needs_update",
                                              "hint": "--netns-dummy: no node this workspace may use can make a job's own network namespace with dummy interfaces yet: that needs a Linux node with k2-node 0.46.14"}})
        return self.reply(404, {"error": {"code": "not_found", "hint": u.path}})

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

last_key() { tail -1 "$LOG" | python3 -c 'import json,sys; v=json.load(sys.stdin); x=v["body"].get(sys.argv[1]); print(json.dumps(x))' "$1"; }
reqs() { wc -l < "$LOG" | tr -d ' '; }

echo "== bad values: exit 2, nothing sent =="
for args in "--network on" "--network no-lan" "--network bogus" "--network" "--netns-dummy sewlane0" "--netns-dummy =10.0.0.1/32" "--netns-dummy a=10.0.0.1" \
    "--netns-dummy a=10.0.0.1/32 --netns-dummy b=10.0.0.2/32 --netns-dummy c=10.0.0.3/32 --netns-dummy d=10.0.0.4/32 --netns-dummy e=10.0.0.5/32"; do
    n0="$(reqs)"
    set +e
    # shellcheck disable=SC2086
    "$K2" compute run --no-source --workspace builder $args -- true >/dev/null 2>&1; rc=$?
    set -e
    expect_eq "$args exits 2" "$rc" "2"
    expect_eq "$args sends nothing" "$(reqs)" "$n0"
done
expect_has "--network on says a job only narrows" "$("$K2" compute run --no-source --network on -- true 2>&1 || true)" "a job can only narrow its network"

echo "== the body =="
set +e
err="$("$K2" compute run --no-source --workspace builder --network none -- true 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "--network none sends network" "$(last_key network)" '"none"'
expect_eq "... and no dummies" "$(last_key netnsDummies)" "null"
expect_eq "job_network_needs_update exits 3" "$rc" "3"
expect_has "the code" "$err" "job_network_needs_update"
expect_has "the daemon's hint" "$err" "k2-node 0.46.14"
"$K2" compute run --no-source --workspace builder --netns-dummy sewlane0=10.254.254.1/32 --netns-dummy v6=fd00::1/128 -- true >/dev/null 2>&1 || true
expect_eq "--netns-dummy sends them as given" "$(last_key netnsDummies)" '["sewlane0=10.254.254.1/32", "v6=fd00::1/128"]'
expect_eq "--netns-dummy implies --network none" "$(last_key network)" '"none"'
"$K2" compute run --no-source --workspace builder -- true >/dev/null 2>&1 || true
expect_eq "no flag: no network key" "$(last_key network)" "null"
expect_eq "no flag: no netnsDummies key" "$(last_key netnsDummies)" "null"
"$K2" compute test --no-source --workspace builder --network none --netns-dummy sewlane0=10.254.254.1/32 -- -p x >/dev/null 2>&1 || true
expect_eq "compute test passes --network through" "$(last_key network)" '"none"'
expect_eq "compute test passes --netns-dummy through" "$(last_key netnsDummies)" '["sewlane0=10.254.254.1/32"]'

echo "== get =="
out="$("$K2" compute get abcd1234 2>&1)"
expect_has "get: the network the job asked for" "$out" "network: none; dummies: sewlane0 10.254.254.1/32"
json="$("$K2" compute get abcd1234 --json)"
expect_eq "get --json: result.network" "$(printf '%s' "$json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["network"]["dummies"][0]["name"])')" "sewlane0"

echo "== help, study and schema =="
help="$("$K2" compute --help)"
expect_has "help: --network none" "$help" "[--network none]"
expect_has "help: --netns-dummy" "$help" "[--netns-dummy NAME=ADDR/PREFIX]"
study="$("$K2" study compute)"
for c in "NETWORK (--network none, --netns-dummy" job_network_invalid job_network_needs_update job_network_unavailable "10.254.254.1/32" "result.network"; do
    expect_has "study: $c" "$study" "$c"
done
schema="$("$K2" --schema)"
expect_eq "schema: compute run has both flags" "$(printf '%s' "$schema" | python3 -c '
import json, sys
v = json.load(sys.stdin)
run = [c for c in v["commands"] if c["name"] == "compute run"][0]
print(" ".join(sorted(f["name"] for f in run["flags"] if f["name"] in ("--network", "--netns-dummy"))))')" "--netns-dummy --network"

echo
echo "compute network CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
