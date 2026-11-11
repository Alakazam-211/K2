#!/usr/bin/env bash
# 0.46.11 (field report): `k2 app exec <app> -- <command…>` forwards its
# stdin. A piped file or a pipe reaches the command; a terminal or
# /dev/null sends none; --no-stdin sends none; past 64 MiB the CLI refuses
# (exit 2) before any request. Against a stub daemon that runs the argv
# locally with the stdin it was sent, as the App engine does in the wall.
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_* var is unset, the
# daemon is a tiny python stub on loopback.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

pass=0
fail=0
ok()  { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got=$(printf %q "$2") want=$(printf %q "$3"))"; fi; }
assert_contains() { if printf '%s' "$2" | grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }

WORK="$(mktemp -d -t k2-exec-stdin-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

HOME2="$WORK/home"
mkdir -p "$HOME2/.k2"
echo "owner-disk-token-exec-stdin" > "$HOME2/.k2/heartbeat.token"
chmod 600 "$HOME2/.k2/heartbeat.token"

python3 - "$WORK" <<'PYEOF' &
import base64, json, os, subprocess, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def _go(self, method):
        n = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(n) if n else b""
        p = urllib.parse.urlparse(self.path).path
        status, out = 200, {"ok": True}
        if p == "/cli/app/exec":
            v = json.loads(body)
            stdin = base64.b64decode(v["stdin"]) if "stdin" in v else b""
            with open(os.path.join(work, "reqs.jsonl"), "a") as f:
                f.write(json.dumps({"path": p, "keys": sorted(v.keys()), "stdinBytes": len(stdin), "bodyBytes": n}) + "\n")
            r = subprocess.run(v["argv"], input=stdin, capture_output=True)
            text = (r.stdout + r.stderr).decode("utf-8", "replace")
            out = {"ok": True, "app": v["app"], "deployment": "d-0123456789ab", "argv": v["argv"], "exitCode": r.returncode,
                   "timedOut": False, "bytes": len(text), "framed": text.strip()}
        else:
            with open(os.path.join(work, "reqs.jsonl"), "a") as f:
                f.write(json.dumps({"path": p}) + "\n")
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

owner() { env HOME="$HOME2" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" "$K2_CLI" "$@"; }
nreq() { if [ -f "$WORK/reqs.jsonl" ]; then wc -l < "$WORK/reqs.jsonl" | tr -d ' '; else echo 0; fi; }
# The last exec request: "<keys> <stdin bytes>".
last() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if json.loads(l)["path"] == "/cli/app/exec"]
r = rows[-1]
print(",".join(r["keys"]), r["stdinBytes"])' "$WORK/reqs.jsonl"
}

echo "== a pipe reaches the command =="
head -c 1048576 /dev/urandom > "$WORK/one.bin"
out="$(cat "$WORK/one.bin" | owner app exec jobs -- sh -c 'wc -c' || true)"
assert_eq "1 MiB piped in: wc -c says 1048576" "$(printf '%s' "$out" | tr -d ' ')" "1048576"
assert_eq "1 MiB piped in: the request carries it" "$(last)" "app,argv,stdin 1048576"
out="$(owner app exec jobs -- sh -c 'cat | od -An -tx1 | tr -d " \n" | head -c 64' < "$WORK/one.bin")"
assert_eq "a redirected file arrives byte for byte" "$out" "$(od -An -tx1 "$WORK/one.bin" | tr -d ' \n' | head -c 64)"
out="$(owner app exec jobs -- sh -c 'cat | cksum' < "$WORK/one.bin")"
assert_eq "the whole file is the same bytes" "$out" "$(cksum < "$WORK/one.bin")"

echo "== no input =="
out="$(owner app exec jobs -- sh -c 'wc -c' < /dev/null)"
assert_eq "/dev/null: nothing sent" "$(last)" "app,argv 0"
assert_eq "/dev/null: wc -c says 0" "$(printf '%s' "$out" | tr -d ' ')" "0"
out="$(printf 'abc' | owner app exec jobs --no-stdin -- sh -c 'wc -c' || true)"
assert_eq "--no-stdin: nothing sent" "$(last)" "app,argv 0"
out="$(printf '' | owner app exec jobs -- sh -c 'wc -c')"
assert_eq "an empty pipe: wc -c says 0" "$(printf '%s' "$out" | tr -d ' ')" "0"

echo "== the cap =="
before="$(nreq)"
set +e
err="$(head -c $((64 * 1024 * 1024 + 1)) /dev/zero | owner app exec jobs -- true 2>&1)"; rc=$?
set -e
assert_eq "past 64 MiB: exit 2" "$rc" "2"
assert_contains "past 64 MiB: says the cap" "$err" "64 MiB"
assert_eq "past 64 MiB: no request" "$(nreq)" "$before"

echo "== help =="
assert_eq "help names --no-stdin" "$(owner app exec --help 2>&1 | grep -c -- "--no-stdin" || true)" "1"

echo
echo "app_exec_stdin: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
