#!/usr/bin/env bash
# 0.46.8 — a login someone else makes has a TEMPORARY password.
# `k2 users add` sends mustChangePassword:true unless --keep-password (then
# false); `k2 users set-password` sends the field only with --keep-password
# (the daemon decides the default: temporary, except the owner's own login)
# and says what the daemon answered. The help names the flag. A local HTTP
# stub stands in for the daemon and logs each POST body; never touches a
# real ~/.k2.

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
        *"$needle"*) echo "  PASS: $label"; pass=$((pass + 1)) ;;
        *) echo "  FAIL: $label (missing $(printf %q "$needle") in $(printf %q "$hay"))" >&2; fail=$((fail + 1)) ;;
    esac
}

SANDBOX="$(mktemp -d -t k2-users-temp-XXXXXX)"
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
unset K2_HOOK_SOCK K2SO_HOOK_SOCK K2SO_PORT K2SO_HOOK_TOKEN K2_PROJECT_PATH K2SO_PROJECT_PATH K2_CELL K2_API_CELL || true

STUB_PY="$SANDBOX/stub.py"
BODIES="$SANDBOX/bodies.jsonl"
cat >"$STUB_PY" <<'PY'
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = os.environ["BODIES"]

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        path = self.path.split("?", 1)[0]
        raw = self.rfile.read(int(self.headers.get("Content-Length") or 0)).decode()
        body = json.loads(raw) if raw else {}
        with open(LOG, "a") as f:
            f.write(json.dumps({"path": path, "body": body}) + "\n")
        if path == "/cli/users/add":
            out = {"success": True, "mustChangePassword": body.get("mustChangePassword", True)}
        elif path == "/cli/users/set-password":
            # The daemon's default: temporary unless the body says false.
            out = {"success": True, "mustChangePassword": body.get("mustChangePassword", True)}
        else:
            out = {"success": True}
        data = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        data = b'{"users":[]}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(os.environ["PORT"])), H).serve_forever()
PY
STUB_PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
echo "$STUB_PORT" >"$HOME/.k2/heartbeat.port"
echo "fake-token" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"
export K2_HOST=127.0.0.1
export K2_PORT="$STUB_PORT"
export K2_HOOK_TOKEN="fake-token"
BODIES="$BODIES" PORT="$STUB_PORT" python3 "$STUB_PY" &
STUB_PID=$!
up=0
for _i in $(seq 1 50); do
    if curl -sf --connect-timeout 1 "http://127.0.0.1:${STUB_PORT}/cli/users" >/dev/null 2>&1; then
        up=1
        break
    fi
    sleep 0.1
done
[ "$up" = 1 ] || { echo "FAIL: stub daemon never came up" >&2; exit 1; }

# The Nth logged body's mustChangePassword ("absent" when not sent).
body_flag() {
    python3 -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
r = rows[int(sys.argv[2])]
b = r["body"]
print(r["path"], b["username"], json.dumps(b["mustChangePassword"]) if "mustChangePassword" in b else "absent")
' "$BODIES" "$1"
}

echo "== k2 users add: temporary by default =="
out="$(printf 'pass-1234\n' | "$K2_CLI" users add anna --password-stdin)"
assert_eq "add sends mustChangePassword:true" "$(body_flag 0)" "/cli/users/add anna true"
assert_contains "add says it is temporary" "$out" "The password is temporary"

echo "== k2 users add --keep-password =="
out="$(printf 'pass-1234\n' | "$K2_CLI" users add me --password-stdin --keep-password)"
assert_eq "--keep-password sends false" "$(body_flag 1)" "/cli/users/add me false"
assert_contains "add says it is kept" "$out" "The password is kept (not temporary)."

echo "== k2 users set-password =="
out="$(printf 'pass-5678\n' | "$K2_CLI" users set-password anna --password-stdin)"
assert_eq "set-password leaves the default to the daemon" "$(body_flag 2)" "/cli/users/set-password anna absent"
assert_contains "set-password reports temporary" "$out" "It is temporary"
out="$(printf 'pass-5678\n' | "$K2_CLI" users set-password anna --password-stdin --keep-password)"
assert_eq "set-password --keep-password sends false" "$(body_flag 3)" "/cli/users/set-password anna false"
assert_contains "set-password reports kept" "$out" "The password is kept (not temporary)."

echo "== passwords never on argv; usage =="
set +e
"$K2_CLI" users set-password anna --password nope >/dev/null 2>"$SANDBOX/err"
rc=$?
"$K2_CLI" users set-password </dev/null >/dev/null 2>"$SANDBOX/err2"
rc2=$?
set -e
assert_eq "set-password --password exits 2" "$rc" "2"
assert_eq "set-password without a name exits 2" "$rc2" "2"
assert_contains "usage names --keep-password" "$(cat "$SANDBOX/err2")" "[--keep-password]"

echo "== help and schema name the flag =="
help="$("$K2_CLI" users --help 2>&1)"
assert_contains "help: add is temporary" "$help" "TEMPORARY: at their first sign-in"
assert_contains "help: --keep-password" "$help" "--keep-password"
assert_contains "help: set-password" "$help" "k2 users set-password <username> [--password-stdin] [--keep-password]"
schema="$("$K2_CLI" --schema)"
assert_eq "schema users add carries --keep-password" \
    "$(printf '%s' "$schema" | python3 -c 'import json,sys; d=json.load(sys.stdin); c=[c for c in d["commands"] if c["name"]=="users add"][0]; print([f["name"] for f in c["flags"]])')" \
    "['--role', '--password-stdin', '--keep-password']"
assert_eq "schema has users set-password" \
    "$(printf '%s' "$schema" | python3 -c 'import json,sys; d=json.load(sys.stdin); print([c["usage"] for c in d["commands"] if c["name"]=="users set-password"])')" \
    "['k2 users set-password <username> [--password-stdin] [--keep-password]']"

echo
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
