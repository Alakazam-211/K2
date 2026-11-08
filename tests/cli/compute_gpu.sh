#!/usr/bin/env bash
# k2 compute run --gpu and k2 compute grant --gpu (GPU jobs, 0.46.11): CLI
# shapes against a tiny stub daemon on loopback (python3 http.server). No
# real daemon, no ~/.k2, no keychain: HOME is a throwaway dir.
#
# Covers: `--gpu bogus` and `--gpu nvidia` exit 2 with no request; bare
# `--gpu` sends {"vendor": "any", "count": 1}, `--gpu amd` and `--gpu=intel`
# their vendor, no --gpu sends no gpu key; no_gpu_node exits 3 with the
# daemon's hint (both steps); gpu_needs_grant exits 3 and ends with the
# daemon's "Ask with: k2 permission request compute.gpu …" line; grant
# --gpu / --no-gpu send allowGpu and the answer says "GPU jobs allowed";
# `k2 compute test --gpu` passes it through; the help and `k2 study
# compute` (GPU JOBS) name the flag, the codes and the card.

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
# A here-string, not printf | grep -q: under pipefail, grep -q stopping at
# the first match can SIGPIPE printf on a long text (a false "missing").
expect_has() { if grep -Fq -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if grep -Fq -- "$3" <<<"$2"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

SANDBOX="$(mktemp -d /tmp/k2gpu-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

ASK='k2 permission request compute.gpu node=k2n-gpu --why "<what you need it for>"'

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

LOG = sys.argv[1]
ASK = 'k2 permission request compute.gpu node=k2n-gpu --why "<what you need it for>"'

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
        return self.reply(404, {"error": {"code": "not_found", "hint": self.path}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        u = urlparse(self.path)
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": u.path, "body": body}) + "\n")
        if u.path == "/cli/compute/run":
            if body.get("tag") == "needs-grant":
                return self.reply(403, {"error": {"code": "gpu_needs_grant", "node": "k2n-gpu",
                                                  "hint": "--gpu: k2n-gpu runs GPU jobs for this server, but workspace builder's grant there doesn't allow them"},
                                        "permission": {"action": "compute.gpu", "params": {"node": "k2n-gpu"}, "approvers": "owner or admin", "command": ASK}})
            return self.reply(404, {"error": {"code": "no_gpu_node",
                                              "hint": "no node you can use runs GPU jobs for this server. Two people turn it on: the node's owner, at that machine: sudo k2-node admin gpu on --server <this server>; then this server's owner: k2 compute grant <node> --workspace builder --gpu"}})
        if u.path == "/cli/compute/grants/set":
            g = {"maxJobSecs": 7200, "maxParallel": 1, "maxQueued": 10, "maxDiskGb": 60, "allowCi": False, "allowGpu": bool(body.get("allowGpu"))}
            return self.reply(200, {"ok": True, "node": body.get("node"), "workspace": body.get("workspace"), "grant": g, "note": "ok", "resent": 0})
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

last_gpu() { tail -1 "$LOG" | python3 -c 'import json,sys; v=json.load(sys.stdin); print(json.dumps(v["body"].get("gpu"), sort_keys=True))'; }
last_key() { tail -1 "$LOG" | python3 -c 'import json,sys; v=json.load(sys.stdin); x=v["body"].get(sys.argv[1]); print(json.dumps(x))' "$1"; }
reqs() { wc -l < "$LOG" | tr -d ' '; }

echo "== bad values: exit 2, nothing sent =="
for badv in bogus nvidia AMD; do
    n0="$(reqs)"
    set +e
    err="$("$K2" compute run --no-source --workspace builder --gpu "$badv" -- true 2>&1 >/dev/null)"; rc=$?
    set -e
    expect_eq "--gpu $badv exits 2" "$rc" "2"
    expect_eq "--gpu $badv sends nothing" "$(reqs)" "$n0"
done
expect_has "nvidia says why" "$("$K2" compute run --no-source --gpu nvidia -- true 2>&1 || true)" "NVIDIA GPU jobs aren't supported yet"

echo "== the body =="
set +e
err="$("$K2" compute run --no-source --workspace builder --gpu -- true 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "bare --gpu: the request went" "$(tail -1 "$LOG" | python3 -c 'import json,sys; print(json.load(sys.stdin)["path"])')" "/cli/compute/run"
expect_eq "bare --gpu = any, one GPU" "$(last_gpu)" '{"count": 1, "vendor": "any"}'
expect_eq "no_gpu_node exits 3" "$rc" "3"
expect_has "no_gpu_node code" "$err" "no_gpu_node"
expect_has "the hint names the machine owner's step" "$err" "sudo k2-node admin gpu on"
expect_has "the hint names the grant step" "$err" "--gpu"
"$K2" compute run --no-source --workspace builder --gpu amd -- true >/dev/null 2>&1 || true
expect_eq "--gpu amd" "$(last_gpu)" '{"count": 1, "vendor": "amd"}'
"$K2" compute run --no-source --workspace builder --gpu=intel -- true >/dev/null 2>&1 || true
expect_eq "--gpu=intel" "$(last_gpu)" '{"count": 1, "vendor": "intel"}'
"$K2" compute run --no-source --workspace builder -- true >/dev/null 2>&1 || true
expect_eq "no --gpu: no gpu key" "$(last_gpu)" "null"
"$K2" compute test --no-source --workspace builder --gpu amd -- -p x >/dev/null 2>&1 || true
expect_eq "compute test passes --gpu amd through" "$(last_gpu)" '{"count": 1, "vendor": "amd"}'

echo "== gpu_needs_grant: exit 3, ends with the card line =="
set +e
err="$("$K2" compute run --no-source --workspace builder --gpu --tag needs-grant -- true 2>&1 >/dev/null)"; rc=$?
set -e
expect_eq "gpu_needs_grant exits 3" "$rc" "3"
expect_has "gpu_needs_grant code" "$err" "gpu_needs_grant"
expect_eq "it ends with the ask" "$(printf '%s\n' "$err" | tail -1)" "Ask with: $ASK"

echo "== grant --gpu / --no-gpu =="
out="$("$K2" compute grant k2n-gpu --workspace builder --gpu)"
expect_eq "grant --gpu sends allowGpu true" "$(last_key allowGpu)" "true"
expect_has "the answer says so" "$out" "GPU jobs allowed"
out="$("$K2" compute grant k2n-gpu --workspace builder --no-gpu)"
expect_eq "grant --no-gpu sends allowGpu false" "$(last_key allowGpu)" "false"
expect_not "no GPU in the answer" "$out" "GPU jobs allowed"
"$K2" compute grant k2n-gpu --workspace builder --max-parallel 2 >/dev/null
expect_eq "re-granting limits sends no allowGpu (keeps it)" "$(last_key allowGpu)" "null"

echo "== help and study =="
help="$("$K2" compute --help)"
expect_has "help: run --gpu" "$help" "[--gpu [any|amd|intel]]"
expect_has "help: grant --gpu" "$help" "--gpu: this workspace may send --gpu jobs"
expect_has "help: the card" "$help" "k2 permission request compute.gpu"
study="$("$K2" study compute)"
expect_has "study: section" "$study" "GPU JOBS (0.46.11, Linux nodes with the job wall"
for c in no_gpu_node gpu_needs_grant gpu_busy gpu_unavailable K2_GPU_RENDER "sudo k2-node admin gpu on" "--label gpu=amd"; do
    expect_has "study: $c" "$study" "$c"
done

echo
echo "compute gpu CLI: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
