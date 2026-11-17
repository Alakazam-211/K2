#!/usr/bin/env bash
# Signed approvals S1: `k2 approve request / status / cancel` against a stub
# daemon (the daemon's own behaviour is tested in k2-daemon's approve::).
#
# Covers: the payload file goes byte for byte as payload_b64; --to rides
# along only when given; --wait polls /cli/approve/wait until a final state;
# --out writes the record exactly as the phone wrote it (recordText); exit 0
# approved, 1 declined; a refusal exits 3 and names its code; usage errors
# exit 2 before any request; help prints with no daemon. Hermetic: HOME is a
# throwaway dir, every K2_*/K2SO_* var is unset, the daemon is a python stub.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

WORK="$(mktemp -d -t k2-approve-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

HOME2="$WORK/home"
mkdir -p "$HOME2/.k2"
echo "owner-disk-token-approve" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

# The record text with its own spacing: --out must keep it byte for byte.
RECORD='{"v":1, "payload_b64":"AA==","key_id":"91ac1312245d73df57fe662bd9502777017b542cf30a9d9569957841c5f58a19","alg":"ES256","sig_b64":"MEUCIQ=="}'
printf '%s' "$RECORD" > "$WORK/record.txt"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
record = open(os.path.join(work, "record.txt")).read()
polls = {"n": 0}

def view(state):
    v = {"ok": True, "id": "ap_0123456789abcdef", "state": state, "person": "rosson", "kind": "com.example.refund",
         "rp": "shop@test", "nonce": "00112233445566778899aabbccddeeff", "answerBy": 1790510400, "device": None,
         "record": None, "recordText": None, "card": None}
    if state == "approved":
        v.update(device="iPhone 16", record=json.loads(record), recordText=record)
    return v

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        n = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(n) if n else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        with open(os.path.join(work, "reqs.jsonl"), "a") as f:
            f.write(json.dumps({"method": method, "path": parsed.path, "body": body.decode("utf-8", "replace"),
                                "id": (q.get("id") or [""])[0]}) + "\n")
        p = parsed.path
        status, out = 200, view("waiting")
        final = open(os.path.join(work, "final")).read().strip() if os.path.exists(os.path.join(work, "final")) else "approved"
        if p == "/cli/approve/request" and os.path.exists(os.path.join(work, "refuse")):
            status, out = 403, {"ok": False, "error": {"code": "approve_request_off", "hint": "this agent may not ask people for signed approvals yet"}}
        elif p == "/cli/approve/request":
            out = view("waiting")
        elif p == "/cli/approve/wait":
            polls["n"] += 1
            out = view("waiting" if polls["n"] < 2 else final)
        elif p == "/cli/approve/status":
            out = view(final) if q.get("id") else {"ok": True, "requests": [view("waiting")]}
        elif p == "/cli/approve/cancel":
            out = view("cancelled")
        else:
            status, out = 404, {"ok": False, "error": {"code": "not_found", "hint": p}}
        raw = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        self._go("GET")

    def do_POST(self):
        self._go("POST")

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 100); do [ -s "$WORK/stub.port" ] && break; sleep 0.05; done
STUB_PORT="$(cat "$WORK/stub.port")"

k2() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" "$@"; }
nreq() { if [ -f "$WORK/reqs.jsonl" ]; then wc -l < "$WORK/reqs.jsonl" | tr -d ' '; else echo 0; fi; }
# The last request to PATH: its JSON body, keys sorted (or its id query).
last() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if json.loads(l)["path"] == sys.argv[2]]
r = rows[-1]
print(r["method"], json.dumps(json.loads(r["body"]), sort_keys=True) if r["body"] else "id=" + r["id"])' "$WORK/reqs.jsonl" "$1"
}
count() { grep -c "\"path\": \"$1\"" "$WORK/reqs.jsonl" || true; }

# A payload with a zero byte and non-ASCII bytes: it must arrive unchanged.
printf 'K2-SIGNED-APPROVAL-1\000{"note":"\303\234"}' > "$WORK/p.bin"
WANT_B64="$(python3 -c 'import base64,sys; print(base64.b64encode(open(sys.argv[1],"rb").read()).decode())' "$WORK/p.bin")"

echo "== help, no daemon =="
out="$(env HOME="$WORK/nohome" K2_PORT=1 "$K2_CLI" approve --help 2>&1)"
assert_contains "help: request" "$out" "k2 approve request --payload <file> [--to <login>] [--wait] [--out <file>] [--json]"
assert_contains "help: only the person answers" "$out" "Only the person answers, on their own phone."

echo "== request =="
out="$(k2 approve request --payload "$WORK/p.bin")"
assert_eq "the bytes go as payload_b64, no --to" "$(last /cli/approve/request)" "POST {\"payload_b64\": \"$WANT_B64\"}"
assert_contains "prints the id and state" "$out" "ap_0123456789abcdef  waiting"
assert_contains "says how to check" "$out" "k2 approve status ap_0123456789abcdef"
k2 approve request --payload "$WORK/p.bin" --to gina >/dev/null
assert_eq "--to rides along" "$(last /cli/approve/request)" "POST {\"payload_b64\": \"$WANT_B64\", \"to\": \"gina\"}"

echo "== request --wait --out =="
set +e
out="$(k2 approve request --payload "$WORK/p.bin" --wait --out "$WORK/approval.json" 2>&1)"; rc=$?
set -e
assert_eq "approved: exit 0" "$rc" "0"
assert_eq "polled until final" "$(count /cli/approve/wait)" "2"
assert_eq "waits on its own id" "$(last /cli/approve/wait)" "GET id=ap_0123456789abcdef"
assert_eq "--out: the record exactly as the phone wrote it" "$(cat "$WORK/approval.json")" "$RECORD"
assert_contains "says where it went" "$out" "Wrote the approval record to $WORK/approval.json"

echo "== declined =="
echo declined > "$WORK/final"
rm -f "$WORK/reqs.jsonl"
set +e
out="$(k2 approve request --payload "$WORK/p.bin" --wait --out "$WORK/no.json" 2>&1)"; rc=$?
set -e
assert_eq "declined: exit 1" "$rc" "1"
assert_contains "declined: says so" "$out" "declined"
[ ! -e "$WORK/no.json" ] && ok "declined: no record written" || bad "declined: no record written"
rm -f "$WORK/final"

echo "== status / cancel =="
out="$(k2 approve status ap_0123456789abcdef --out "$WORK/s.json")"
assert_eq "status: the id query" "$(last /cli/approve/status)" "GET id=ap_0123456789abcdef"
assert_eq "status --out writes the record" "$(cat "$WORK/s.json")" "$RECORD"
out="$(k2 approve status)"
assert_contains "status: the list" "$out" "ap_0123456789abcdef  waiting"
out="$(k2 approve cancel ap_0123456789abcdef)"
assert_eq "cancel: the request" "$(last /cli/approve/cancel)" 'POST {"id": "ap_0123456789abcdef"}'
assert_contains "cancel: says so" "$out" "Cancelled: ap_0123456789abcdef"

echo "== refusals and usage =="
touch "$WORK/refuse"
set +e
err="$(k2 approve request --payload "$WORK/p.bin" 2>&1)"; rc=$?
set -e
assert_eq "a refusal exits 3" "$rc" "3"
assert_contains "a refusal names its code" "$err" "approve_request_off"
rm -f "$WORK/refuse"
before="$(nreq)"
for args in "request" "request --payload $WORK/missing.bin" "request --payload $WORK/p.bin --out x.json" "cancel" "nope"; do
    set +e
    # shellcheck disable=SC2086
    k2 approve $args >/dev/null 2>&1; rc=$?
    set -e
    assert_eq "usage error exits 2: k2 approve $args" "$rc" "2"
done
assert_eq "usage errors send nothing" "$(nreq)" "$before"

echo
echo "approve request CLI: $pass passed, $fail failed"
[ "$fail" = 0 ]
