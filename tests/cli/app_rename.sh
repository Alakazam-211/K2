#!/usr/bin/env bash
# prd-skin-rename-app-cli-settings-v1 §6 — name cut: k2 app canonical,
# leftover k2 skin / k2 skin-token. No daemon required except a local
# HTTP stub for the user-list alias. Never touch real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
K2="$K2_CLI"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
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
            fail=$((fail + 1))
            ;;
        *)
            echo "  PASS: $label"
            pass=$((pass + 1))
            ;;
    esac
}
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

SANDBOX="$(mktemp -d -t k2-app-rename-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

export HOME="$SANDBOX"
mkdir -p "$HOME/.k2"
echo "1" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
export K2_HOST=127.0.0.1
export K2_PORT=1
export K2_HOOK_TOKEN="fake-token"
unset K2_HOOK_SOCK K2SO_HOOK_SOCK K2SO_PORT K2SO_HOOK_TOKEN || true

run_k2() {
    env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
        HOME="$HOME" \
        K2_HOST="${K2_HOST}" \
        K2_PORT="${K2_PORT}" \
        K2_HOOK_TOKEN="${K2_HOOK_TOKEN}" \
        "$K2_CLI" "$@"
}

echo "== bash -n =="
if bash -n "$K2_CLI"; then
    echo "  PASS: bash -n cli/k2"
    pass=$((pass + 1))
else
    echo "  FAIL: bash -n cli/k2" >&2
    fail=$((fail + 1))
fi

echo "== k2 app --help is roster usage, not desktop updater =="
set +e
app_help="$(run_k2 app --help 2>&1)"
app_help_rc=$?
set -e
assert_eq "k2 app --help exit 0" "$app_help_rc" "0"
assert_contains "app help family" "$app_help" "Usage: k2 app"
assert_contains "app help guests/roles/tokens" "$app_help" "Guests, roles, and tokens"
assert_contains "app help desktop updates" "$app_help" "k2 update --app"
assert_not_contains "app help not updater" "$app_help" "Checking for app updates"
assert_contains "app help full-name" "$app_help" "k2 app user full-name"
assert_contains "app help list full name column" "$app_help" "full name"

echo "== leftover k2 skin teach line is stderr-only =="
set +e
skin_help="$(run_k2 skin --help 2>/dev/null)"
skin_help_err="$(run_k2 skin --help 2>&1 >/dev/null)"
skin_user_help="$(run_k2 skin user --help 2>&1)"
skin_user_help_rc=$?
set -e
assert_contains "skin --help stderr teach" "$skin_help_err" "k2 skin is now k2 app — this command still works."
assert_not_contains "skin --help stdout has no teach" "$skin_help" "k2 skin is now k2 app"
assert_contains "k2 skin user --help contains k2 app" "$skin_user_help" "k2 app"
assert_eq "k2 skin user --help exit 0" "$skin_user_help_rc" "0"

echo "== leftover k2 skin-token teaches k2 app token =="
set +e
tok_help_err="$(run_k2 skin-token --help 2>&1 >/dev/null)"
tok_help="$(run_k2 skin-token --help 2>&1)"
set -e
assert_contains "skin-token teach" "$tok_help_err" "k2 skin-token is now k2 app token — this command still works."
assert_contains "skin-token help is app token" "$tok_help" "k2 app token"

echo "== k2 app update is unknown, not the updater; app-update stays deprecated =="
set +e
app_update="$(run_k2 app update 2>&1)"
app_update_rc=$?
app_hyphen="$(run_k2 app-update 2>&1)"
app_hyphen_rc=$?
app_token_hyphen="$(run_k2 app-token 2>&1)"
app_token_hyphen_rc=$?
set -e
assert_eq "k2 app update exit 1" "$app_update_rc" "1"
assert_contains "k2 app update is usage" "$app_update" "Usage: k2 app"
assert_not_contains "k2 app update not updater" "$app_update" "Checking for app updates"
assert_eq "k2 app-update still deprecated" "$app_hyphen_rc" "1"
assert_contains "app-update points at update --app" "$app_hyphen" "k2 update --app"
assert_eq "k2 app-token is unknown" "$app_token_hyphen_rc" "1"
assert_contains "app-token unknown command" "$app_token_hyphen" "Unknown command: app-token"

echo "== schema family app + leftover skin / skin-token; no app-token =="
schema="$("$K2_CLI" --schema 2>/dev/null || true)"
assert_contains "schema name app" "$schema" '"name": "app"'
assert_contains "schema name app token" "$schema" '"name": "app token"'
assert_contains "schema leftover skin" "$schema" '"name": "skin"'
assert_contains "schema leftover skin-token" "$schema" '"name": "skin-token"'
assert_not_contains "schema has no app-token family" "$schema" '"name": "app-token"'
assert_contains "schema --skin flag stays" "$schema" '"name": "--skin"'
assert_contains "schema --skin sentence" "$schema" "Static HTML host (an app)"

echo "== k2 publish run --help still documents --skin =="
set +e
pub_help="$(run_k2 publish --help 2>&1)"
pub_run_help="$(run_k2 publish run --help 2>&1)"
set -e
assert_contains "publish --help --skin" "$pub_help" "--skin"
assert_contains "publish --help static HTML" "$pub_help" "Static HTML host (an app)"
assert_contains "publish run --help --skin" "$pub_run_help" "--skin"
assert_not_contains "publish has no --app flag" "$pub_help" "--app ["

echo "== routes / prefix unchanged in cli/k2 =="
src="$(cat "$K2_CLI")"
assert_contains "login route" "$src" "/cli/skin/login"
assert_contains "k2skn_ prefix" "$src" "k2skn_"
assert_contains "skin-tokens route" "$src" "/cli/skin-tokens"
assert_contains "hydra leftover verb" "$src" "k2 app hydra"
assert_contains "front-door leftover verb" "$src" "k2 app front-door"

echo "== k2 app user list and k2 skin user list both 0 (stub daemon) =="
STUB_PY="$SANDBOX/stub.py"
cat >"$STUB_PY" <<'PY'
import json
from http.server import BaseHTTPRequestHandler, HTTPServer

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/cli/skin/users":
            body = b'{"users":[]}'
        elif path == "/cli/skin/roles":
            body = b'{"roles":[]}'
        else:
            self.send_response(404)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(__import__("os").environ["PORT"])), H).serve_forever()
PY
STUB_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
export K2_PORT="$STUB_PORT"
echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"
PORT="$STUB_PORT" python3 "$STUB_PY" &
STUB_PID=$!
for _i in 1 2 3 4 5 6 7 8 9 10; do
    if curl -sf --connect-timeout 1 "http://127.0.0.1:${STUB_PORT}/cli/skin/users" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

set +e
app_list_out="$(run_k2 app user list 2>/dev/null)"
app_list_err="$(run_k2 app user list 2>&1 >/dev/null)"
app_list_rc=$?
skin_list_out="$(run_k2 skin user list 2>/dev/null)"
skin_list_err="$(run_k2 skin user list 2>&1 >/dev/null)"
skin_list_rc=$?
set -e
assert_eq "k2 app user list exit 0" "$app_list_rc" "0"
assert_eq "k2 skin user list exit 0" "$skin_list_rc" "0"
assert_eq "stdout roster identical" "$app_list_out" "$skin_list_out"
assert_contains "empty roster stdout" "$app_list_out" "No skin users."
assert_not_contains "app list no teach on stderr" "$app_list_err" "k2 skin is now k2 app"
assert_contains "skin list teach on stderr" "$skin_list_err" "k2 skin is now k2 app — this command still works."
assert_not_contains "teach not on stdout" "$skin_list_out" "k2 skin is now k2 app"

echo "== k2 app user full-name help and list column (stub daemon) =="
set +e
fn_help="$(run_k2 app user full-name --help 2>&1)"
fn_help_rc=$?
set -e
assert_eq "k2 app user full-name --help exit 0" "$fn_help_rc" "0"
assert_contains "full-name help usage" "$fn_help" "k2 app user full-name <username> [--name <text>]"
assert_contains "schema full-name" "$schema" '"name": "app user full-name"'

kill "$STUB_PID" 2>/dev/null || true
wait "$STUB_PID" 2>/dev/null || true
STUB_PID=""
STUB_PY2="$SANDBOX/stub-full-name.py"
cat >"$STUB_PY2" <<'PY'
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

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/cli/skin/users":
            self._send(200, json.dumps({"users": [{
                "username": "ada",
                "fullName": "Ada Lovelace",
                "roleName": None,
                "email": "ada@clinic.com",
                "hasPassword": True,
                "passwordHash": "secret-hash",
            }]}))
        elif path == "/cli/skin/roles":
            self._send(200, json.dumps({"roles": []}))
        else:
            self._send(404, json.dumps({"error": "not found"}))

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        n = int(self.headers.get("Content-Length") or "0")
        raw = self.rfile.read(n)
        if path != "/cli/skin/users/full-name":
            self._send(404, json.dumps({"error": "not found"}))
            return
        body = json.loads(raw.decode() or "{}")
        self._send(200, json.dumps({
            "username": body.get("username") or "",
            "fullName": body.get("fullName") or None,
        }))

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(__import__("os").environ["PORT"])), H).serve_forever()
PY
STUB_PORT2="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
export K2_PORT="$STUB_PORT2"
echo "$STUB_PORT2" >"$HOME/.k2/heartbeat.port"
PORT="$STUB_PORT2" python3 "$STUB_PY2" &
STUB_PID=$!
for _i in 1 2 3 4 5 6 7 8 9 10; do
    if curl -sf --connect-timeout 1 "http://127.0.0.1:${STUB_PORT2}/cli/skin/users" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

set +e
named_list="$(run_k2 app user list 2>/dev/null)"
named_list_rc=$?
set_out="$(run_k2 app user full-name ada --name "Ada/Lovelace: MD" 2>/dev/null)"
set_rc=$?
clear_out="$(run_k2 app user full-name ada 2>/dev/null)"
clear_rc=$?
empty_name_out="$(run_k2 app user full-name ada --name "" 2>/dev/null)"
empty_name_rc=$?
set -e
assert_eq "named list exit 0" "$named_list_rc" "0"
assert_contains "list shows username" "$named_list" "ada"
assert_contains "list shows full name" "$named_list" "Ada Lovelace"
assert_not_contains "list hides password hash" "$named_list" "secret-hash"
assert_not_contains "list hides k2skn" "$named_list" "k2skn_"
assert_eq "full-name set exit 0" "$set_rc" "0"
assert_contains "full-name set text" "$set_out" "set to Ada/Lovelace: MD"
assert_eq "full-name omit clears exit 0" "$clear_rc" "0"
assert_contains "full-name omit clears" "$clear_out" "cleared"
assert_eq "full-name empty --name clears exit 0" "$empty_name_rc" "0"
assert_contains "full-name empty --name clears" "$empty_name_out" "cleared"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
