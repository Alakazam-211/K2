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
BODY_FILE="$WORK/body"

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
        if mode == "stale_cli":
            d = dict(STATUS); d["cli"] = {"path": "/usr/local/bin/k2", "state": "stale", "version": "0.40.148",
                "rootOwned": True, "daemonCliVersion": "0.46.3", "lastAttempt": 1,
                "lastResult": {"result": "needs_helper", "reason": "the root helper must speak protocol 9"}}
            return self._send(200, json.dumps(d))
        if mode == "current_cli":
            d = dict(STATUS); d["cli"] = {"path": "/usr/local/bin/k2", "state": "current", "version": "0.46.3",
                "rootOwned": True, "daemonCliVersion": "0.46.3", "lastAttempt": 1,
                "lastResult": {"result": "refreshed", "from": "0.40.148", "to": "0.46.3"}}
            return self._send(200, json.dumps(d))
        if mode == "update_dropin_installed":
            return self._send(200, json.dumps({"heal": {"result": "updated", "from": "0.46.3", "to": "0.46.5"},
                "cli": {"result": "current"},
                "dropin": {"result": "installed", "unit": "k2-daemon.service", "scope": "system", "restartNeeded": True,
                    "restart": "k2 daemon restart from a root shell (or: sudo systemctl restart k2-daemon.service)",
                    "why": "the daemon unit started without this drop-in: RuntimeDirectory=k2 and KillMode=process take effect when the unit next starts (one restart, when convenient)"},
                "status": STATUS}))
        if mode == "update_dropin_current":
            return self._send(200, json.dumps({"heal": {"result": "up_to_date", "version": "0.46.5"}, "cli": {"result": "current"},
                "dropin": {"result": "current", "unit": "k2-daemon.service", "scope": "system", "restartNeeded": False},
                "status": STATUS}))
        if mode == "update_dropin_failed":
            return self._send(200, json.dumps({"heal": {"result": "retry", "reason": "offline"}, "cli": {"result": "current"},
                "dropin": {"result": "failed", "unit": "k2-daemon.service", "scope": "system", "reason": "helper_needed: protocol 4"},
                "status": STATUS}))
        if mode == "update_dropin_na":
            return self._send(200, json.dumps({"heal": {"result": "up_to_date", "version": "0.46.5"}, "cli": {"result": "current"},
                "dropin": {"result": "not_applicable", "reason": "not a systemd unit"}, "status": STATUS}))
        if mode == "update_newer_daemon":
            return self._send(200, json.dumps({"target": "0.46.4",
                "warnings": ["the installed daemon binary is v0.46.5, newer than the helper this fetches (v0.46.4): run `k2 root-helper update --version 0.46.5` (or `--match-installed`) to install its helper now; otherwise the daemon fetches it after it restarts on v0.46.5"],
                "heal": {"result": "up_to_date", "version": "0.46.4"}, "cli": {"result": "current"},
                "dropin": {"result": "not_applicable"}, "status": STATUS}))
        if mode == "no_command":
            d = dict(STATUS); d["installCommand"] = None
            return self._send(200, json.dumps(d))
        return self._send(200, json.dumps(STATUS))

    def do_GET(self):
        self._answer()

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        open(os.environ["BODY_FILE"], "w").write(self.rfile.read(n).decode())
        self._answer()

    def log_message(self, *_a):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo ok >"$MODE_FILE"
MODE_FILE="$MODE_FILE" BODY_FILE="$BODY_FILE" PORT="$PORT" python3 "$WORK/stub.py" &
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

echo "== status: the /usr/local/bin/k2 doctor line =="
echo stale_cli >"$MODE_FILE"
run root-helper status
assert_eq "stale cli: exit 0" "$rc" "0"
assert_contains "stale cli: names the file, version and owner" "$out" "cli        : /usr/local/bin/k2 v0.40.148 (root-owned) — older than the daemon (v0.46.3)"
assert_contains "stale cli: why it is not refreshed yet" "$out" "cli refresh: needs_helper the root helper must speak protocol 9"
echo current_cli >"$MODE_FILE"
run root-helper status
assert_contains "current cli: says current" "$out" "cli        : /usr/local/bin/k2 v0.46.3 — current"
assert_contains "current cli: the last refresh" "$out" "cli refresh: refreshed 0.40.148 -> 0.46.3"
assert_absent "cli: no traceback" "$err" "Traceback"

echo "== update: the unit drop-in converged after the helper update (0.46.5) =="
echo update_dropin_installed >"$MODE_FILE"
run root-helper update
assert_eq "dropin installed: exit 0" "$rc" "0"
assert_contains "dropin installed: written through the helper" "$out" "unit dropin: k2-daemon.service written now (through the helper; systemd reloaded)"
assert_contains "dropin installed: the one restart" "$out" "restart    : once, when convenient: k2 daemon restart from a root shell (or: sudo systemctl restart k2-daemon.service)"
assert_contains "dropin installed: why" "$out" "RuntimeDirectory=k2 and KillMode=process take effect when the unit next starts"
assert_absent "dropin installed: no traceback" "$err" "Traceback"
echo update_dropin_current >"$MODE_FILE"
run root-helper update
assert_eq "dropin current: exit 0" "$rc" "0"
assert_contains "dropin current: says current" "$out" "unit dropin: k2-daemon.service current"
assert_absent "dropin current: no restart line" "$out" "restart    :"
echo update_dropin_failed >"$MODE_FILE"
run root-helper update
assert_eq "dropin failed: exit 0 (the heal answered)" "$rc" "0"
assert_contains "dropin failed: the reason" "$out" "unit dropin: k2-daemon.service not written: helper_needed: protocol 4"
assert_absent "dropin failed: no restart line" "$out" "restart    :"
echo update_dropin_na >"$MODE_FILE"
run root-helper update
assert_eq "dropin n/a: exit 0" "$rc" "0"
assert_absent "dropin n/a: no line off systemd" "$out" "unit dropin"
assert_absent "dropin: no traceback" "$err" "Traceback"

echo "== update: --version / --match-installed and the newer-daemon warning (0.46.6) =="
echo update_dropin_current >"$MODE_FILE"
run root-helper update
assert_eq "update plain: exit 0" "$rc" "0"
assert_eq "update plain: empty body" "$(cat "$BODY_FILE")" "{}"
run root-helper update --version 0.46.5
assert_eq "update --version: exit 0" "$rc" "0"
assert_eq "update --version: sends the version" "$(cat "$BODY_FILE")" '{"version": "0.46.5"}'
run root-helper update --match-installed
assert_eq "update --match-installed: exit 0" "$rc" "0"
assert_eq "update --match-installed: sends matchInstalled" "$(cat "$BODY_FILE")" '{"matchInstalled": true}'
run root-helper update --version 0.46.5 --match-installed
assert_eq "update both: usage exit 2" "$rc" "2"
assert_contains "update both: says not both" "$err" "not both"
run root-helper update --version
assert_eq "update --version without value: exit 2" "$rc" "2"
echo update_newer_daemon >"$MODE_FILE"
run root-helper update
assert_eq "newer daemon: exit 0" "$rc" "0"
assert_contains "newer daemon: which helper" "$out" "helper for : v0.46.4"
assert_contains "newer daemon: the warning" "$out" "warning    : the installed daemon binary is v0.46.5"
assert_contains "newer daemon: the fix" "$out" "k2 root-helper update --version 0.46.5"
assert_absent "newer daemon: no traceback" "$err" "Traceback"

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
