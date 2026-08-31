#!/usr/bin/env bash
# `k2 agent list` transport class: local_transport_denied vs daemon_unreachable.
# Injects fake curl/OS outcomes (PATH wrap) plus a real closed-port refuse
# and a stub daemon. Fail loud; no skip-if-missing. No live K2 daemon.
# Run with: bash tests/cli/agent_list_transport.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"

[ -f "$K2" ] || { echo "FAIL: $K2 not found" >&2; exit 1; }
[ -x "$K2" ] || { echo "FAIL: $K2 not executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

pass=0
fail=0

assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}

assert_contains() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*)
            echo "  PASS: $label"
            pass=$((pass + 1))
            ;;
        *)
            echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2
            fail=$((fail + 1))
            ;;
    esac
}

assert_absent() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*)
            echo "  FAIL: $label unexpected $(printf %q "$needle") in $(printf %q "$hay")" >&2
            fail=$((fail + 1))
            ;;
        *)
            echo "  PASS: $label"
            pass=$((pass + 1))
            ;;
    esac
}

assert_err_code() {
    local label="$1" err="$2" want="$3"
    local got
    got="$(printf '%s' "$err" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw)
except Exception as e:
    sys.stderr.write("not JSON: %r (%s)\n" % (raw[:400], e))
    sys.exit(2)
err = d.get("error")
if not isinstance(err, dict):
    sys.stderr.write("missing error object: %r\n" % (d,))
    sys.exit(2)
print(err.get("code", ""))
')" || got="__parse_failed__"
    assert_eq "$label" "$got" "$want"
}

assert_err_hint_has() {
    local label="$1" err="$2" needle="$3"
    local hint
    hint="$(printf '%s' "$err" | python3 -c '
import json, sys
d = json.loads(sys.stdin.read())
print((d.get("error") or {}).get("hint", ""))
')" || hint=""
    assert_contains "$label" "$hint" "$needle"
}

WORKDIR="$(mktemp -d -t k2-agent-list-transport-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

mkdir -p "$WORKDIR/home/.k2" "$WORKDIR/bin" "$WORKDIR/project"

# Fake curl: OS/curl outcomes without needing sandbox EPERM.
cat > "$WORKDIR/bin/curl" << 'EOF'
#!/bin/bash
mode="${K2_TEST_CURL_MODE:-}"
case "$mode" in
    eperm)
        echo '* connect to 127.0.0.1 port 9 failed: Operation not permitted' >&2
        echo 'curl: (7) Failed to connect to 127.0.0.1 port 9: Operation not permitted' >&2
        exit 7
        ;;
    eacces)
        echo '* connect to 127.0.0.1 port 9 failed: Permission denied' >&2
        echo 'curl: (7) Failed to connect to 127.0.0.1 port 9: Permission denied' >&2
        exit 7
        ;;
    tmpdir)
        echo 'curl: (23) Failed to create temporary file' >&2
        exit 23
        ;;
    ipv6)
        echo '* Trying [::1]:9...' >&2
        echo '* Immediate connect fail for [::1]:9: Address family not supported by protocol' >&2
        echo 'curl: (7) Failed to connect to ::1 port 9: Address family not supported' >&2
        exit 7
        ;;
    generic7)
        echo 'curl: (7) Failed to connect to 127.0.0.1 port 9 after 0 ms: Couldn'"'"'t connect to server' >&2
        exit 7
        ;;
    *)
        echo "FAIL: fake curl invoked without K2_TEST_CURL_MODE (args: $*)" >&2
        exit 99
        ;;
esac
EOF
chmod +x "$WORKDIR/bin/curl"

run_list() {
    # $1 = PATH prefix (may be empty); remaining env via the environment.
    local path_prefix="${1:-}"
    shift || true
    local path="$PATH"
    [ -n "$path_prefix" ] && path="${path_prefix}:$PATH"
    env HOME="$WORKDIR/home" \
        K2_PORT="${K2_PORT:-9}" \
        K2_HOOK_TOKEN="${K2_HOOK_TOKEN:-fake}" \
        K2_PROJECT_PATH="$WORKDIR/project" \
        PATH="$path" \
        "$K2" agent list "$@"
}

echo "== bash -n =="
if bash -n "$K2"; then
    echo "  PASS: bash -n cli/k2"
    pass=$((pass + 1))
else
    echo "  FAIL: bash -n cli/k2" >&2
    fail=$((fail + 1))
fi

echo "== TCP lock: _uds_eligible omits /cli/agent/list =="
# shellcheck disable=SC1090
eval "$(sed -n '/^_uds_eligible()/,/^}/p' "$K2")"
if _uds_eligible "/cli/agent/list"; then
    echo "  FAIL: /cli/agent/list must not be UDS-eligible (TCP cli_request)" >&2
    fail=$((fail + 1))
else
    echo "  PASS: /cli/agent/list not UDS-eligible"
    pass=$((pass + 1))
fi
if _uds_eligible "/cli/heartbeat/list"; then
    echo "  PASS: /cli/heartbeat/list still UDS-eligible"
    pass=$((pass + 1))
else
    echo "  FAIL: _uds_eligible heartbeat regression" >&2
    fail=$((fail + 1))
fi

echo "== EPERM → local_transport_denied (exit 1) =="
set +e
out="$(K2_TEST_CURL_MODE=eperm run_list "$WORKDIR/bin" --json 2>"$WORKDIR/eperm.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/eperm.err")"
assert_eq "eperm exit" "$rc" "1"
assert_err_code "eperm code" "$err" "local_transport_denied"
assert_err_hint_has "eperm hint restriction" "$err" "local restriction"
assert_err_hint_has "eperm hint unobserved" "$err" "daemon health unobserved"
assert_absent "eperm not unreachable" "$err" "daemon_unreachable"
assert_absent "eperm not 'is it running'" "$err" "is it running?"
assert_eq "eperm stdout empty" "$out" ""

echo "== EACCES → local_transport_denied (exit 1) =="
set +e
out="$(K2_TEST_CURL_MODE=eacces run_list "$WORKDIR/bin" --json 2>"$WORKDIR/eacces.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/eacces.err")"
assert_eq "eacces exit" "$rc" "1"
assert_err_code "eacces code" "$err" "local_transport_denied"
assert_err_hint_has "eacces hint" "$err" "daemon health unobserved"
assert_absent "eacces not unreachable" "$err" "daemon_unreachable"

echo "== TMPDIR fail → local_transport_denied =="
set +e
out="$(K2_TEST_CURL_MODE=tmpdir run_list "$WORKDIR/bin" --json 2>"$WORKDIR/tmpdir.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/tmpdir.err")"
assert_eq "tmpdir exit" "$rc" "1"
assert_err_code "tmpdir code" "$err" "local_transport_denied"

echo "== unusable IPv6 loopback → local_transport_denied =="
set +e
out="$(K2_TEST_CURL_MODE=ipv6 run_list "$WORKDIR/bin" --json 2>"$WORKDIR/ipv6.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/ipv6.err")"
assert_eq "ipv6 exit" "$rc" "1"
assert_err_code "ipv6 code" "$err" "local_transport_denied"

echo "== curl 7 with no OS class → daemon_unreachable =="
set +e
out="$(K2_TEST_CURL_MODE=generic7 run_list "$WORKDIR/bin" --json 2>"$WORKDIR/generic7.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/generic7.err")"
assert_eq "generic7 exit" "$rc" "1"
assert_err_code "generic7 code" "$err" "daemon_unreachable"
assert_err_hint_has "generic7 hint" "$err" "is it running?"
assert_absent "generic7 not denied" "$err" "local_transport_denied"

echo "== connection refused (real curl, closed port) → daemon_unreachable =="
python3 -c '
import socket, sys
s = socket.socket()
s.bind(("127.0.0.1", 0))
port = s.getsockname()[1]
s.close()
open(sys.argv[1], "w").write(str(port))
' "$WORKDIR/closed.port"
CLOSED_PORT="$(cat "$WORKDIR/closed.port")"
set +e
out="$(K2_PORT="$CLOSED_PORT" run_list "" --json 2>"$WORKDIR/refused.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/refused.err")"
assert_eq "refused exit" "$rc" "1"
assert_err_code "refused code" "$err" "daemon_unreachable"
assert_err_hint_has "refused hint" "$err" "is it running?"
assert_absent "refused not denied" "$err" "local_transport_denied"
assert_eq "refused stdout empty" "$out" ""

echo "== stub daemon: success + JSON error passthrough =="
python3 - "$WORKDIR" << 'PY' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

root = sys.argv[1]
n = {"i": 0}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        return
    def do_GET(self):
        path = urlparse(self.path).path
        n["i"] += 1
        if path != "/cli/agent/list":
            body = json.dumps({"ok": False, "error": {"code": "not_found", "hint": "stub " + path}}).encode()
        elif n["i"] == 1:
            body = json.dumps({
                "ok": True,
                "agents": [{
                    "name": "ada",
                    "mode": "k2",
                    "enabled": True,
                    "live": False,
                    "path": "/tmp/ada",
                }],
            }).encode()
        else:
            body = json.dumps({
                "ok": False,
                "error": {"code": "gated", "hint": "agent list is owner-gated — ask your human"},
            }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

httpd = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(root, "stub.port"), "w").write(str(httpd.server_address[1]))
httpd.serve_forever()
PY
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true
for _ in $(seq 1 50); do
    [ -f "$WORKDIR/stub.port" ] && break
    sleep 0.05
done
[ -f "$WORKDIR/stub.port" ] || { echo "FAIL: stub daemon never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORKDIR/stub.port")"

set +e
out="$(K2_PORT="$STUB_PORT" run_list "" 2>"$WORKDIR/ok.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/ok.err")"
assert_eq "success exit" "$rc" "0"
assert_contains "success header" "$out" "NAME"
assert_contains "success ada" "$out" "ada"
assert_eq "success stderr empty" "$err" ""

set +e
out="$(K2_PORT="$STUB_PORT" run_list "" --json 2>"$WORKDIR/gated.err")"
rc=$?
set -e
err="$(cat "$WORKDIR/gated.err")"
assert_eq "daemon error exit" "$rc" "1"
assert_err_code "daemon error passthrough" "$err" "gated"
assert_err_hint_has "daemon error hint kept" "$err" "owner-gated"
assert_absent "daemon error not unreachable" "$err" "daemon_unreachable"
assert_absent "daemon error not denied" "$err" "local_transport_denied"
assert_eq "daemon error stdout empty" "$out" ""

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
