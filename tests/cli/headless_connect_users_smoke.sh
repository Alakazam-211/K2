#!/usr/bin/env bash
# Headless Connect onboarding — CLI surface smoke (no live Supabase/daemon).
#
# Covers:
#   1. Catalog: users → users-admin; connect → connect-account (id, locked)
#   2. publish stays publish (not remapped through connect)
#   3. Help / usage text for users + connect (daemon-less)
#   4. Schema mentions users + connect login
#   5. bare `k2 connect` / old publish-under-connect pointers
#   6. Password refused on argv for users add
#   7. bash -n on cli/k2
#
# No daemon build. HOME sandboxed. Never touches the real ~/.k2.

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
    # Not a pipe: grep -q under pipefail treats SIGPIPE as failure.
    if grep -Fq "$needle" <<<"$hay"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "${hay:0:200}"))" >&2
        fail=$((fail + 1))
    fi
}
assert_ok() {
    local label="$1"; shift
    if "$@"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label" >&2
        fail=$((fail + 1))
    fi
}
assert_exit() {
    local label="$1" want="$2"; shift 2
    set +e
    "$@" >/tmp/k2-hc-out.$$ 2>/tmp/k2-hc-err.$$
    local rc=$?
    set -e
    if [ "$rc" = "$want" ]; then
        echo "  PASS: $label (exit $rc)"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (exit $rc want $want)" >&2
        echo "    stdout: $(head -c 200 /tmp/k2-hc-out.$$)" >&2
        echo "    stderr: $(head -c 200 /tmp/k2-hc-err.$$)" >&2
        fail=$((fail + 1))
    fi
}

# Sandbox HOME so connect/status never touch the real account file.
TEST_HOME="$(mktemp -d)"
trap 'rm -rf "$TEST_HOME" /tmp/k2-hc-out.$$ /tmp/k2-hc-err.$$ 2>/dev/null' EXIT
export HOME="$TEST_HOME"
mkdir -p "$HOME/.k2"

echo "== bash -n =="
assert_ok "bash -n cli/k2" bash -n "$K2_CLI"

echo "== catalog =="
# shellcheck disable=SC1090
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
assert_eq "users → users-admin" "$(_cli_tool_id_for_verb users)" "users-admin"
assert_eq "connect → connect-account" "$(_cli_tool_id_for_verb connect)" "connect-account"
assert_eq "publish → publish" "$(_cli_tool_id_for_verb publish)" "publish"
assert_eq "connect-account mode id" "$(_cli_tool_default_mode connect-account)" "id"
assert_eq "users-admin mode id" "$(_cli_tool_default_mode users-admin)" "id"
assert_ok "connect-account locked" _cli_tool_is_locked connect-account
assert_ok "users-admin locked" _cli_tool_is_locked users-admin

# Auth: external (no hook sock) keeps owner; in-cell ID keeps scoped.
OWNER="owner-disk-token-aaaa"
SCOPED="sess99.scoped-secret-bbbb"
TOKEN="stale"
SCOPED_TOKEN="$SCOPED"
DISK_OWNER_TOKEN="$OWNER"
unset K2_HOOK_SOCK K2SO_HOOK_SOCK || true
_cli_apply_auth_for_verb connect
# No sock → TOKEN unchanged from prior discovery path; apply leaves TOKEN as-is for id.
# Seed TOKEN as owner first (external path):
TOKEN="$OWNER"
_cli_apply_auth_for_verb connect
assert_eq "connect external keeps owner" "$TOKEN" "$OWNER"

TOKEN="stale"
SCOPED_TOKEN="$SCOPED"
DISK_OWNER_TOKEN="$OWNER"
K2_HOOK_SOCK="/tmp/k2-hc-fake.sock"
_cli_apply_auth_for_verb connect
assert_eq "connect in-cell keeps scoped" "$TOKEN" "$SCOPED"
unset K2_HOOK_SOCK

TOKEN="stale"
SCOPED_TOKEN="$SCOPED"
DISK_OWNER_TOKEN="$OWNER"
K2_HOOK_SOCK="/tmp/k2-hc-fake.sock"
_cli_apply_auth_for_verb users
assert_eq "users in-cell keeps scoped" "$TOKEN" "$SCOPED"
unset K2_HOOK_SOCK

echo "== help / usage (daemon-less via connect skip gate) =="
# connect skips the connection gate so help works without PORT/TOKEN.
out="$("$K2_CLI" connect --help 2>&1 || true)"
assert_contains "connect help has login" "$out" "connect login"
assert_contains "connect help has status" "$out" "connect status"
assert_contains "connect help has logout" "$out" "connect logout"

assert_exit "bare connect → usage exit 2" 2 "$K2_CLI" connect
assert_contains "bare connect mentions login" "$(cat /tmp/k2-hc-err.$$ 2>/dev/null; cat /tmp/k2-hc-out.$$ 2>/dev/null)" "connect login"

assert_exit "old connect subdomain → publish pointer" 2 "$K2_CLI" connect subdomain list
assert_contains "subdomain pointer" "$(cat /tmp/k2-hc-err.$$)" "k2 publish"

assert_exit "connect status without account" 0 "$K2_CLI" connect status
assert_contains "status shows unpaired" "$(cat /tmp/k2-hc-out.$$)" "Paired subdomain"

assert_exit "connect logout clears nothing" 0 "$K2_CLI" connect logout

# users needs daemon (no skip gate) — without PORT/TOKEN should fail the conn gate.
assert_exit "users without daemon fails gate" 1 env -u K2_PORT -u K2_HOOK_TOKEN -u K2SO_PORT -u K2SO_HOOK_TOKEN "$K2_CLI" users --help
# Actually --help still hits the gate because only connect/publish/schema skip it.
# Force help via parsing: with fake PORT+TOKEN help still needs functions.
# Provide fake port/token so the gate passes; help is pure local.
assert_exit "users help with fake auth" 0 \
    env K2_PORT=9 K2_HOOK_TOKEN=fake "$K2_CLI" users --help
assert_contains "users help has add" "$(cat /tmp/k2-hc-out.$$; cat /tmp/k2-hc-err.$$)" "users add"
assert_contains "users help has unlock" "$(cat /tmp/k2-hc-out.$$; cat /tmp/k2-hc-err.$$)" "k2 users unlock"

assert_exit "users unlock --help is group usage" 0 \
    env K2_PORT=9 K2_HOOK_TOKEN=fake "$K2_CLI" users unlock --help
assert_contains "users unlock --help lists unlock" "$(cat /tmp/k2-hc-out.$$; cat /tmp/k2-hc-err.$$)" "k2 users unlock"
assert_exit "users unlock missing name" 2 \
    env K2_PORT=9 K2_HOOK_TOKEN=fake "$K2_CLI" users unlock
assert_contains "users unlock usage" "$(cat /tmp/k2-hc-err.$$)" "k2 users unlock <username>"
assert_exit "users unlock unknown subcommand lists unlock" 2 \
    env K2_PORT=9 K2_HOOK_TOKEN=fake "$K2_CLI" users nosuch
assert_contains "unknown users subcommand usage has unlock" "$(cat /tmp/k2-hc-err.$$)" "k2 users unlock"

assert_exit "users add refuses --password on argv" 2 \
    env K2_PORT=9 K2_HOOK_TOKEN=fake "$K2_CLI" users add alice --password secret
assert_contains "password-on-argv message" "$(cat /tmp/k2-hc-err.$$)" "must not be passed"

echo "== schema =="
schema="$(env K2_PORT=9 K2_HOOK_TOKEN=fake "$K2_CLI" --schema 2>/dev/null || true)"
# --schema skips gate; no env needed but fine either way
schema="$("$K2_CLI" --schema 2>/dev/null || true)"
assert_contains "schema has connect login" "$schema" '"name": "connect login"'
assert_contains "schema has users add" "$schema" '"name": "users add"'
assert_contains "schema has users unlock" "$schema" '"name": "users unlock"'
assert_contains "schema users usage lists unlock" "$schema" "k2 users [list|add|audit|unlock]"
assert_contains "schema has connect" "$schema" '"name": "connect"'
assert_contains "schema has users" "$schema" '"name": "users"'
# Full-document json.loads is not required: pre-existing mail-flag
# descriptions embed \Seen/\Flagged (invalid JSON escapes) elsewhere in
# the static manifest. Our entries are already checked via assert_contains.
assert_contains "schema connect login has --token flag" "$schema" '"name": "--token"'

echo "== users unlock does not treat cleared:false as failure (stub) =="
STUB_PY="$TEST_HOME/unlock_stub.py"
cat >"$STUB_PY" <<'PY'
import json
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        raw = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        n = int(self.headers.get("Content-Length") or "0")
        raw = self.rfile.read(n)
        if path != "/cli/users/unlock":
            self._send(404, json.dumps({"error": "not found"}))
            return
        body = json.loads(raw.decode() or "{}")
        name = body.get("username") or ""
        if name == "nope":
            self._send(403, json.dumps({"error": "invalid or missing token"}))
            return
        cleared = name == "baden"
        self._send(200, json.dumps({"ok": True, "username": name, "cleared": cleared}))

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(__import__("os").environ["PORT"])), H).serve_forever()
PY
STUB_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"
PORT="$STUB_PORT" python3 "$STUB_PY" &
STUB_PID=$!
trap 'kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; rm -rf "$TEST_HOME" /tmp/k2-hc-out.$$ /tmp/k2-hc-err.$$ 2>/dev/null' EXIT
for _i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if curl -sf -o /dev/null --connect-timeout 1 -X POST "http://127.0.0.1:${STUB_PORT}/cli/users/unlock" -d '{}'; then
        break
    fi
    sleep 0.05
done

run_users() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
        HOME="$HOME" K2_HOST=127.0.0.1 K2_PORT="$STUB_PORT" K2_HOOK_TOKEN=fake \
        "$K2_CLI" users "$@"
}
set +e
none_out="$(run_users unlock other 2>/tmp/k2-hc-err.$$)"
none_rc=$?
cleared_out="$(run_users unlock baden 2>>/tmp/k2-hc-err.$$)"
cleared_rc=$?
json_out="$(run_users unlock other --json 2>>/tmp/k2-hc-err.$$)"
json_rc=$?
bad_out="$(run_users unlock nope --json 2>/tmp/k2-hc-err.$$)"
bad_rc=$?
set -e
assert_eq "cleared false exit 0" "$none_rc" "0"
assert_eq "no lock line" "$none_out" "No lock for other"
assert_eq "cleared true exit 0" "$cleared_rc" "0"
assert_eq "lock cleared line" "$cleared_out" "Lock cleared for baden"
assert_eq "json cleared false exit 0" "$json_rc" "0"
assert_contains "json body" "$json_out" '"cleared": false'
assert_contains "json ok" "$json_out" '"ok": true'
if [ "$bad_rc" -ne 0 ]; then
    echo "  PASS: refused token exit non-zero ($bad_rc)"
    pass=$((pass + 1))
else
    echo "  FAIL: refused token exit 0" >&2
    fail=$((fail + 1))
fi
assert_contains "refused token prints daemon error" "$(cat /tmp/k2-hc-err.$$)" "invalid or missing token"
case "$bad_out" in
    *'"ok": true'*)
        echo "  FAIL: --json printed success body before the error check" >&2
        fail=$((fail + 1))
        ;;
    *)
        echo "  PASS: --json did not print the body on error"
        pass=$((pass + 1))
        ;;
esac

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
