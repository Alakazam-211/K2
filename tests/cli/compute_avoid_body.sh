#!/usr/bin/env bash
# k2 compute run / test --avoid: what reaches the daemon, against a tiny
# stub daemon on loopback (python3 http.server). No real daemon, no ~/.k2,
# no keychain: HOME is a throwaway dir.
#
# Covers: a long submit line (--repo, --os, --arch, --sha, --needs, --cpus,
# --mem, --timeout, five --avoid, more --avoid from a variable, --tag,
# --detach) sends every --avoid, in order; the same with --any and with no
# --repo; `k2 compute test` passes them through; and `k2 compute get`
# names what an auto job avoids on its placement line (and says nothing
# when it avoids nothing).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2avoid-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

LOG = sys.argv[1]

def job(avoid):
    j = {"id": "abcd1234" * 4, "short": "abcd1234", "node": "k2n12-vin", "state": "done", "exitCode": 0, "argv": ["true"],
         "placement": {"mode": "auto", "policy": "v2", "node": "k2n12-vin", "code": "starts_now_idle", "why": "starts now · idle", "fits": "3 nodes fit"}}
    if avoid is not None:
        j["avoid"] = avoid
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
    def do_GET(self):
        u = urlparse(self.path)
        if u.path == "/cli/compute/jobs/get":
            which = parse_qs(u.query).get("job", [""])[0]
            return self.reply(200, job(["k2n11-vin", "z13flow"] if which.startswith("avoid") else []))
        return self.reply(404, {"error": {"code": "not_found", "hint": self.path}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        u = urlparse(self.path)
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "body": body}) + "\n")
        return self.reply(404, {"error": {"code": "no_node_fits", "hint": "stub: nothing runs here"}})

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

last_avoid() { tail -1 "$LOG" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["path"], json.dumps(v["body"].get("avoid")))'; }

echo "== run: every --avoid reaches the daemon =="
NA="--avoid k2n9-vin --avoid k2n10-vin --avoid k2n8-vin"
# shellcheck disable=SC2086  # NA is a list of flags, as a submit script keeps it
"$K2" compute run --repo k2-mpc-engine --os linux --arch x86_64 --sha 3a121820 --needs rust@1.99.1+clippy+rustfmt --cpus 2 --mem 4G --timeout 2h \
    --avoid k2n11-vin --avoid z13flow --avoid k2n1-m4 --avoid k2n2-m1 --avoid k2n3-m1 $NA --tag rc3:check-3a12 --detach -- sh -c 'true' >/dev/null 2>&1 || true
expect_eq "the long submit line sends all eight" "$(last_avoid)" \
    '/cli/compute/run ["k2n11-vin", "z13flow", "k2n1-m4", "k2n2-m1", "k2n3-m1", "k2n9-vin", "k2n10-vin", "k2n8-vin"]'
"$K2" compute run --sha 3a121820 --arch x86_64 --os linux --cpus 2 --mem 8G --timeout 1h --needs rust@stable --avoid k2n11-vin --tag rc3:x -- true >/dev/null 2>&1 || true
expect_eq "no --repo, attached: the same" "$(last_avoid)" '/cli/compute/run ["k2n11-vin"]'
"$K2" compute run --any --no-source --avoid k2n11-vin -- true >/dev/null 2>&1 || true
expect_eq "--any: the same" "$(last_avoid)" '/cli/compute/run ["k2n11-vin"]'
"$K2" compute run --no-source -- true >/dev/null 2>&1 || true
expect_eq "no --avoid: no avoid key" "$(last_avoid)" '/cli/compute/run null'

echo "== test: passes --avoid through =="
"$K2" compute test --no-source --avoid k2n11-vin --avoid k2n1 -- -p x >/dev/null 2>&1 || true
expect_eq "compute test --avoid" "$(last_avoid)" '/cli/compute/run ["k2n11-vin", "k2n1"]'

echo "== --cache-required reaches the daemon =="
last_cache() { tail -1 "$LOG" | python3 -c 'import json,sys; b=json.load(sys.stdin)["body"]; print(b.get("cache"), json.dumps(b.get("cacheRequired")))'; }
"$K2" compute run --no-source --cache corpus --cache-required -- true >/dev/null 2>&1 || true
expect_eq "run --cache-required" "$(last_cache)" 'corpus true'
"$K2" compute run --no-source --cache corpus -- true >/dev/null 2>&1 || true
expect_eq "no --cache-required: no key" "$(last_cache)" 'corpus null'
"$K2" compute test --no-source --cache corpus --cache-required -- -p x >/dev/null 2>&1 || true
expect_eq "compute test passes --cache-required" "$(last_cache)" 'corpus true'

echo "== get: names what an auto job avoids =="
out="$("$K2" compute get avoid1234 2>&1)"
expect_has "the placement line names the avoided nodes" "$out" "placed on k2n12-vin (auto, v2): starts now · idle · 3 nodes fit · avoids k2n11-vin, z13flow"
out="$("$K2" compute get plain1234 2>&1)"
expect_has "a job that avoids nothing: the placement line" "$out" "placed on k2n12-vin (auto, v2): starts now · idle · 3 nodes fit"
expect_not "... with no avoids" "$out" "avoids"

echo
echo "compute_avoid_body: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
