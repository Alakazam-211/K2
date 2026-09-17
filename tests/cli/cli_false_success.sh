#!/usr/bin/env bash
# Appa C — CLI false-successes: empty-array set -u, spawn fail-loud, triage args.
#
# Pure checks. Fake PORT/TOKEN. Sandbox HOME. Never skip: if a check
# cannot run, FAIL.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

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
            echo "  FAIL: $label (missing $(printf %q "$needle"))" >&2
            fail=$((fail + 1))
            ;;
    esac
}
assert_not_contains() {
    local label="$1" hay="$2" needle="$3"
    case "$hay" in
        *"$needle"*)
            echo "  FAIL: $label (found $(printf %q "$needle"))" >&2
            echo "    hay: $(printf '%s' "$hay" | head -c 400)" >&2
            fail=$((fail + 1))
            ;;
        *)
            echo "  PASS: $label"
            pass=$((pass + 1))
            ;;
    esac
}

# Isolate from a live K2 cell (K2_HOOK_SOCK / K2_PORT would hit the
# production daemon or exit 3 before the empty-array sites).
run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
        -u K2SO_PORT -u K2SO_HOOK_TOKEN \
        -u K2_PROJECT_PATH -u K2SO_PROJECT_PATH \
        HOME="$HOME" \
        K2_HOST=127.0.0.1 \
        K2_PORT="${K2_PORT}" \
        K2_HOOK_TOKEN="${K2_HOOK_TOKEN}" \
        "$K2_CLI" "$@"
}

# ── 0. Syntax ────────────────────────────────────────────────────────
echo "== bash -n =="
if bash -n "$K2_CLI"; then
    echo "  PASS: bash -n cli/k2"
    pass=$((pass + 1))
else
    echo "  FAIL: bash -n cli/k2" >&2
    fail=$((fail + 1))
fi

# ── 1. Sandbox HOME + fake connection ────────────────────────────────
WORK="$(mktemp -d -t k2-cli-false-success-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/ws"
echo "1" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
export K2_PORT=1
export K2_HOOK_TOKEN="fake-token"

# ── 2. workspace create/open: empty extra[@] must not unbound ────────
echo "== workspace create/open empty extra[@] =="
set +e
create_out="$(run_k2 workspace create "$WORK/newws" 2>&1)"
create_rc=$?
set -e
assert_not_contains "workspace create: no unbound variable" "$create_out" "unbound variable"

set +e
open_out="$(run_k2 workspace open "$WORK/ws" 2>&1)"
open_rc=$?
set -e
assert_not_contains "workspace open: no unbound variable" "$open_out" "unbound variable"

# ── 3. skin role create without --agent (still optional) ─────────────
echo "== skin role create (no --agent) =="
set +e
role_out="$(run_k2 skin role create skintest-a 2>&1)"
role_rc=$?
set -e
assert_not_contains "skin role create: no unbound variable" "$role_out" "unbound variable"
assert_not_contains "skin role create: --agent still optional" "$role_out" "needs ≥1 --agent"

set +e
role_upd_out="$(run_k2 skin role update skintest-a --clear 2>&1)"
set -e
assert_not_contains "skin role update --clear: no unbound variable" "$role_upd_out" "unbound variable"

set +e
user_rooms_out="$(run_k2 skin user rooms someuser 2>&1)"
set -e
assert_not_contains "skin user rooms: no unbound variable" "$user_rooms_out" "unbound variable"

set +e
tok_rooms_out="$(run_k2 skin-token rooms someid --clear 2>&1)"
set -e
assert_not_contains "skin-token rooms --clear: no unbound variable" "$tok_rooms_out" "unbound variable"

# ── 4. workspace triage extra args ───────────────────────────────────
echo "== workspace triage extra args =="
set +e
triage_out="$(run_k2 workspace triage /tmp/nope 2>&1)"
triage_rc=$?
set -e
assert_eq "workspace triage /tmp/nope exit 2" "$triage_rc" "2"
assert_contains "triage usage mentions inbox" "$triage_out" "inbox"

# ── 5. workspace --help ──────────────────────────────────────────────
echo "== workspace --help =="
set +e
ws_help="$(run_k2 workspace --help 2>&1)"
ws_help_rc=$?
set -e
assert_eq "workspace --help exit 0" "$ws_help_rc" "0"
assert_contains "workspace help has inbox list" "$ws_help" "inbox list"
assert_not_contains "workspace help has no pending work" "$ws_help" "pending work"

# ── 6. terminal spawn fail-loud ──────────────────────────────────────
echo "== terminal spawn handler path =="
set +e
python3 - "$K2_CLI" <<'PY'
import pathlib, re, sys
src = pathlib.Path(sys.argv[1]).read_text()
m = re.search(r'^cmd_terminal_spawn\(\) \{(.*?)^\}\n', src, re.M | re.S)
if not m:
    print('FAIL: cmd_terminal_spawn not found', file=sys.stderr)
    sys.exit(1)
body = m.group(1)
idx_fail = body.find('cli_print_or_fail')
idx_ok = body.find('Terminal spawned')
if idx_fail < 0:
    print('FAIL: cmd_terminal_spawn must call cli_print_or_fail', file=sys.stderr)
    sys.exit(1)
if idx_ok < 0:
    print('FAIL: cmd_terminal_spawn missing Terminal spawned line', file=sys.stderr)
    sys.exit(1)
if idx_fail > idx_ok:
    print('FAIL: cli_print_or_fail must run before Terminal spawned', file=sys.stderr)
    sys.exit(1)
if 'terminal spawn returned no body' not in body:
    print('FAIL: empty resp must fail before success echo', file=sys.stderr)
    sys.exit(1)
sys.exit(0)
PY
spawn_path_rc=$?
set -e
if [ "$spawn_path_rc" -eq 0 ]; then
    echo "  PASS: spawn handler calls cli_print_or_fail before success"
    pass=$((pass + 1))
else
    echo "  FAIL: spawn handler path check" >&2
    fail=$((fail + 1))
fi

echo "== terminal spawn behavioral (error JSON) =="
cat >"$WORK/stub.py" <<'PYEOF'
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if body:
            self.wfile.write(body)
    def do_GET(self):
        if "/cli/terminal/spawn" in self.path:
            self._send(403, b'{"error":"forbidden"}')
        else:
            self._send(200, b'{"ok":true}')
    def do_POST(self):
        self._send(403, b'{"error":{"code":"forbidden","hint":"stub"}}')
    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
port = srv.server_address[1]
open(sys.argv[1] + "/stub.port", "w").write(str(port))
while True:
    srv.handle_request()
PYEOF
python3 "$WORK/stub.py" "$WORK" >"$WORK/stub.out" 2>"$WORK/stub.err" &
STUB_PID=$!
for i in $(seq 1 50); do
    [ -f "$WORK/stub.port" ] && break
    if ! kill -0 "$STUB_PID" 2>/dev/null; then
        break
    fi
    sleep 0.05
done
STUB_PORT="$(cat "$WORK/stub.port" 2>/dev/null || echo "")"
if [ -z "$STUB_PORT" ]; then
    echo "  FAIL: stub daemon did not start" >&2
    echo "    stub.err: $(head -c 400 "$WORK/stub.err" 2>/dev/null)" >&2
    echo "    stub.out: $(head -c 200 "$WORK/stub.out" 2>/dev/null)" >&2
    fail=$((fail + 1))
else
    export K2_PORT="$STUB_PORT"
    echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"

    set +e
    spawn_out="$(run_k2 terminal spawn --command true 2>&1)"
    spawn_rc=$?
    set -e
    assert_not_contains "spawn error JSON: no Terminal spawned" "$spawn_out" "Terminal spawned"
    if [ "$spawn_rc" -eq 0 ]; then
        echo "  FAIL: spawn error JSON exit 0 (want non-zero)" >&2
        echo "    out: $(printf '%s' "$spawn_out" | head -c 300)" >&2
        fail=$((fail + 1))
    else
        echo "  PASS: spawn error JSON exit $spawn_rc"
        pass=$((pass + 1))
    fi
    assert_contains "spawn error JSON unwrapped" "$spawn_out" "forbidden"
fi

echo "== terminal spawn behavioral (curl fail / empty) =="
export K2_PORT=1
echo "1" >"$HOME/.k2/heartbeat.port"
set +e
empty_out="$(run_k2 terminal spawn --command true 2>&1)"
empty_rc=$?
set -e
assert_not_contains "spawn curl-fail: no Terminal spawned" "$empty_out" "Terminal spawned"
if [ "$empty_rc" -eq 0 ]; then
    echo "  FAIL: spawn curl-fail exit 0 (want non-zero)" >&2
    echo "    out: $(printf '%s' "$empty_out" | head -c 300)" >&2
    fail=$((fail + 1))
else
    echo "  PASS: spawn curl-fail exit $empty_rc"
    pass=$((pass + 1))
fi

# ── Summary ──────────────────────────────────────────────────────────
echo ""
echo "cli_false_success: $pass passed, $fail failed"
if [ "$fail" -gt 0 ]; then
    exit 1
fi
exit 0
