#!/usr/bin/env bash
# k2 root-helper — rendering against a stub daemon.
#
# An error body from the daemon (a string `{"error":"route not found"}` or
# `{"error":{"code","hint"}}`), a non-JSON body, or no daemon at all must
# print ONE plain line on stderr and exit 1 — never a Python traceback.
# `--json` passes the body through untouched (exit 1 on an error status).
# `install-command` prints only the command, and fails the same way.
# Fake values only.

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
assert_absent() { if printf '%s' "$2" | grep -Fq -- "$3"; then bad "$1 (unexpected $(printf %q "$3") in $(printf %q "$2"))"; else ok "$1"; fi; }

WORK="$(mktemp -d -t k2-root-helper-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# Never inherit a session's identity or socket from the caller.
for v in $(env | grep -oE '^K2(SO)?_[A-Z0-9_]+'); do unset "$v"; done

export HOME="$WORK/home"
mkdir -p "$HOME/.k2"
MODE_FILE="$WORK/mode"

cat >"$WORK/stub.py" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer

MODE_FILE = os.environ["MODE_FILE"]
STATUS = {
    "state": "missing", "path": "/usr/local/libexec/k2-root-helper",
    "version": None, "protocol": None, "daemonVersion": "0.0.0-test",
    "legacy": {"mail": "absent", "pg": False}, "rootSetup": "copy-command",
    "installCommand": "curl -fsSL https://example.test/install | sudo sh",
}

class H(BaseHTTPRequestHandler):
    def _send(self, code, raw, ctype="application/json"):
        data = raw.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _answer(self):
        mode = open(MODE_FILE).read().strip()
        if mode == "string_error":
            return self._send(404, '{"error":"route not found"}')
        if mode == "object_error":
            return self._send(403, json.dumps({"error": {"code": "owner_only", "hint": "an owner/admin step"}}))
        if mode == "error_200":
            return self._send(200, '{"error":"route not found"}')
        if mode == "not_json":
            return self._send(502, "<html>bad gateway</html>", "text/html")
        if mode == "list_200":
            return self._send(200, "[1, 2]")
        if mode == "no_command":
            d = dict(STATUS); d["installCommand"] = None
            return self._send(200, json.dumps(d))
        return self._send(200, json.dumps(STATUS))

    def do_GET(self):
        self._answer()

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(n)
        self._answer()

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo ok >"$MODE_FILE"
MODE_FILE="$MODE_FILE" PORT="$PORT" python3 "$WORK/stub.py" &
STUB_PID=$!
for _i in $(seq 1 50); do
    if curl -sf --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/root-helper/status" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
curl -sf --connect-timeout 1 "http://127.0.0.1:${PORT}/cli/root-helper/status" >/dev/null \
    || { echo "FAIL: stub daemon did not start" >&2; exit 1; }

k2() { env HOME="$HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token "$K2_CLI" "$@"; }
run() {
    set +e
    out="$(k2 "$@" 2>"$WORK/err")"
    rc=$?
    err="$(cat "$WORK/err")"
    set -e
}

echo "== status: healthy answer =="
echo ok >"$MODE_FILE"
run root-helper status
assert_eq "ok: exit 0" "$rc" "0"
assert_contains "ok: state line" "$out" "state      : missing"
assert_contains "ok: install command shown" "$out" "curl -fsSL https://example.test/install | sudo sh"
assert_absent "ok: no traceback" "$err" "Traceback"

for sub in status update rollback; do
    echo "== $sub: string error body =="
    echo string_error >"$MODE_FILE"
    run root-helper "$sub"
    assert_eq "$sub string error: exit 1" "$rc" "1"
    assert_contains "$sub string error: plain line" "$err" "k2 root-helper: error: route not found"
    assert_absent "$sub string error: no traceback" "$err" "Traceback"
    assert_eq "$sub string error: nothing on stdout" "$out" ""

    echo "== $sub: object error body =="
    echo object_error >"$MODE_FILE"
    run root-helper "$sub"
    assert_eq "$sub object error: exit 1" "$rc" "1"
    assert_contains "$sub object error: code + hint" "$err" "k2 root-helper: error: owner_only: an owner/admin step"
    assert_absent "$sub object error: no traceback" "$err" "Traceback"
done

echo "== status: error body even with HTTP 200 =="
echo error_200 >"$MODE_FILE"
run root-helper status
assert_eq "error_200: exit 1" "$rc" "1"
assert_contains "error_200: plain line" "$err" "k2 root-helper: error: route not found"
assert_absent "error_200: no traceback" "$err" "Traceback"

echo "== status: non-JSON and non-object bodies =="
echo not_json >"$MODE_FILE"
run root-helper status
assert_eq "not_json: exit 1" "$rc" "1"
assert_contains "not_json: says the status" "$err" "k2 root-helper: error: HTTP 502"
assert_absent "not_json: no traceback" "$err" "Traceback"
echo list_200 >"$MODE_FILE"
run root-helper status
assert_eq "list_200: exit 1" "$rc" "1"
assert_contains "list_200: unexpected answer" "$err" "unexpected answer from the daemon"
assert_absent "list_200: no traceback" "$err" "Traceback"

echo "== --json passes the body through =="
echo string_error >"$MODE_FILE"
run root-helper status --json
assert_eq "--json error: exit 1" "$rc" "1"
assert_eq "--json error: body verbatim" "$out" '{"error":"route not found"}'
echo object_error >"$MODE_FILE"
run root-helper update --json
assert_eq "--json object error: exit 1" "$rc" "1"
assert_eq "--json object error: body verbatim" "$out" '{"error": {"code": "owner_only", "hint": "an owner/admin step"}}'
echo ok >"$MODE_FILE"
run root-helper status --json
assert_eq "--json ok: exit 0" "$rc" "0"
python3 -c 'import json,sys; assert json.loads(sys.argv[1])["state"] == "missing"' "$out" \
    && ok "--json ok: parses" || bad "--json ok: parses ($out)"

echo "== install-command =="
echo ok >"$MODE_FILE"
run root-helper install-command
assert_eq "install-command ok: exit 0" "$rc" "0"
assert_eq "install-command ok: only the command" "$out" "curl -fsSL https://example.test/install | sudo sh"
echo string_error >"$MODE_FILE"
run root-helper install-command
assert_eq "install-command string error: exit 1" "$rc" "1"
assert_contains "install-command string error: plain line" "$err" "k2 root-helper: error: route not found"
assert_absent "install-command string error: no traceback" "$err" "Traceback"
assert_eq "install-command string error: nothing on stdout" "$out" ""
echo object_error >"$MODE_FILE"
run root-helper install-command
assert_eq "install-command object error: exit 1" "$rc" "1"
assert_contains "install-command object error: code" "$err" "owner_only"
assert_absent "install-command object error: no traceback" "$err" "Traceback"
echo not_json >"$MODE_FILE"
run root-helper install-command
assert_eq "install-command not_json: exit 1" "$rc" "1"
assert_absent "install-command not_json: no traceback" "$err" "Traceback"
echo no_command >"$MODE_FILE"
run root-helper install-command
assert_eq "install-command none: exit 1" "$rc" "1"
assert_contains "install-command none: says so" "$err" "no install command"
echo string_error >"$MODE_FILE"
run root-helper install-command --json
assert_eq "install-command --json error: exit 1" "$rc" "1"
assert_eq "install-command --json error: body verbatim" "$out" '{"error":"route not found"}'

echo "== no daemon =="
kill "$STUB_PID" 2>/dev/null || true
wait "$STUB_PID" 2>/dev/null || true
STUB_PID=""
for sub in status install-command; do
    run root-helper "$sub"
    assert_eq "$sub no daemon: exit 1" "$rc" "1"
    assert_contains "$sub no daemon: unreachable" "$err" "daemon unreachable"
    assert_absent "$sub no daemon: no traceback" "$err" "Traceback"
done

echo
echo "root_helper_cli: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
