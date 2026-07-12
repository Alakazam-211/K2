#!/usr/bin/env bash
# Wave 0 / PR-B — CLI must not hand agents the owner token for ID/locked tools.
#
# Covers:
#   1. Pure catalog helpers (tool id + default mode + locked floor) match
#      crates/k2-core/src/cli_tool_policy.rs defaults.
#   2. In a scoped cell ($K2_HOOK_SOCK set), ID verbs keep SCOPED_TOKEN and
#      never re-source ~/.k2/heartbeat.token (owner) into $TOKEN.
#   3. Open verbs still invert to disk owner for TCP ergonomics (COMPAT-58).
#   4. ID verb + scoped cell + empty passport → teaching error, exit 3.
#   5. End-to-end: `k2 mail messages` over TCP records the scoped token, not
#      the owner, when K2_HOOK_SOCK is present.
#
# No daemon build — pure functions are sourced from cli/k2 markers; HTTP is a
# tiny python stub. Never touches the real ~/.k2 (HOME is sandboxed).

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

# ── 1. Pure functions from cli/k2 ─────────────────────────────────────
echo "== pure catalog helpers =="
# shellcheck disable=SC1090
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"

assert_eq "msg → tool id" "$(_cli_tool_id_for_verb msg)" "msg"
assert_eq "talk → msg tool" "$(_cli_tool_id_for_verb talk)" "msg"
assert_eq "mail → mail" "$(_cli_tool_id_for_verb mail)" "mail"
assert_eq "hostmail → mail" "$(_cli_tool_id_for_verb hostmail)" "mail"
assert_eq "sessions → sessions-spawn" "$(_cli_tool_id_for_verb sessions)" "sessions-spawn"
assert_eq "daemon → daemon-admin" "$(_cli_tool_id_for_verb daemon)" "daemon-admin"
assert_eq "api-key → api-key" "$(_cli_tool_id_for_verb api-key)" "api-key"
assert_eq "activity → activity" "$(_cli_tool_id_for_verb activity)" "activity"
assert_eq "skills → skills" "$(_cli_tool_id_for_verb skills)" "skills"
assert_eq "unknown → empty" "$(_cli_tool_id_for_verb totally-unknown)" ""

assert_eq "msg mode id" "$(_cli_tool_default_mode msg)" "id"
assert_eq "mail mode id" "$(_cli_tool_default_mode mail)" "id"
assert_eq "dns mode id" "$(_cli_tool_default_mode dns)" "id"
assert_eq "activity mode open" "$(_cli_tool_default_mode activity)" "open"
assert_eq "skills mode open" "$(_cli_tool_default_mode skills)" "open"
assert_eq "ungoverned mode open" "$(_cli_tool_default_mode "")" "open"

assert_ok "mail is locked" _cli_tool_is_locked mail
assert_ok "dns is locked" _cli_tool_is_locked dns
assert_ok "sessions-spawn is locked" _cli_tool_is_locked sessions-spawn
if _cli_tool_is_locked msg; then
    echo "  FAIL: msg must not be locked" >&2
    fail=$((fail + 1))
else
    echo "  PASS: msg not locked"
    pass=$((pass + 1))
fi

# ── 2. Auth selection (no real network) ───────────────────────────────
echo "== auth selection matrix =="
OWNER="owner-disk-token-aaaa"
SCOPED="sess99.scoped-secret-bbbb"

# ID + hook sock + scoped → TOKEN stays scoped (no owner inversion)
TOKEN="stale"
SCOPED_TOKEN="$SCOPED"
DISK_OWNER_TOKEN="$OWNER"
K2_HOOK_SOCK="/tmp/k2-prb-fake.sock"
_cli_apply_auth_for_verb mail
assert_eq "ID+sock+scoped → TOKEN=scoped" "$TOKEN" "$SCOPED"

# Open + hook sock + disk → TOKEN becomes owner (COMPAT-58 still)
TOKEN="stale"
SCOPED_TOKEN="$SCOPED"
DISK_OWNER_TOKEN="$OWNER"
K2_HOOK_SOCK="/tmp/k2-prb-fake.sock"
_cli_apply_auth_for_verb activity
assert_eq "Open+sock → TOKEN=owner" "$TOKEN" "$OWNER"

# ID + no sock + disk owner already in TOKEN → keep owner (human path)
TOKEN="$OWNER"
SCOPED_TOKEN=""
DISK_OWNER_TOKEN="$OWNER"
unset K2_HOOK_SOCK K2SO_HOOK_SOCK || true
_cli_apply_auth_for_verb mail
assert_eq "ID external human → TOKEN=owner" "$TOKEN" "$OWNER"

# ID + sock + empty scoped → exit 3 teaching error
TOKEN="$OWNER"
SCOPED_TOKEN=""
DISK_OWNER_TOKEN="$OWNER"
K2_HOOK_SOCK="/tmp/k2-prb-fake.sock"
set +e
out="$(_cli_apply_auth_for_verb mail 2>&1)"
rc=$?
set -e
assert_eq "ID+sock+no-passport exit 3" "$rc" "3"
if echo "$out" | grep -q "session ID (passport)"; then
    echo "  PASS: teaching error mentions passport"
    pass=$((pass + 1))
else
    echo "  FAIL: teaching error missing passport text: $out" >&2
    fail=$((fail + 1))
fi
if echo "$out" | grep -q "Settings → CLI Tools"; then
    echo "  PASS: teaching error points at Settings → CLI Tools"
    pass=$((pass + 1))
else
    echo "  FAIL: teaching error missing Settings pointer: $out" >&2
    fail=$((fail + 1))
fi

# ── 3. End-to-end: k2 mail must not send owner token from a scoped cell ─
echo "== e2e mail token (stub daemon) =="
WORK="$(mktemp -d -t k2-prb-auth-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

# Sandbox HOME so DISK_OWNER_TOKEN comes only from our fake heartbeat.token
export HOME="$WORK/home"
mkdir -p "$HOME/.k2"
echo "9999" >"$HOME/.k2/heartbeat.port"   # unused once K2_PORT is set
echo "$OWNER" >"$HOME/.k2/heartbeat.token"
chmod 600 "$HOME/.k2/heartbeat.token"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]

class H(BaseHTTPRequestHandler):
    def _record(self, method):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else b""
        parsed = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(parsed.query)
        token_q = (q.get("token") or [""])[0]
        auth = self.headers.get("Authorization") or ""
        rec = {
            "method": method,
            "path": parsed.path,
            "token_query": token_q,
            "authorization": auth,
            "body": body.decode("utf-8", "replace"),
        }
        with open(os.path.join(work, "last_req.json"), "w") as f:
            json.dump(rec, f)
        data = json.dumps({"ok": True, "messages": [], "count": 0}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self._record("GET")

    def do_POST(self):
        self._record("POST")

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID" 2>/dev/null || true

for _ in $(seq 1 50); do
    [ -f "$WORK/stub.port" ] && break
    sleep 0.05
done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub never bound" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

# Fake per-cell socket path (need not be a real socket — mail is TCP-only today;
# presence of K2_HOOK_SOCK is the scoped-session marker for auth selection).
FAKE_SOCK="$WORK/cell.sock"
# Create a real socket so any UDS probe is harmless if a verb checks -S.
python3 -c "import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1])" "$FAKE_SOCK" 2>/dev/null || touch "$FAKE_SOCK"

set +e
# shellcheck disable=SC2030,SC2031
env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2SO_PORT \
    K2_PORT="$STUB_PORT" \
    K2_HOOK_TOKEN="$SCOPED" \
    K2_HOOK_SOCK="$FAKE_SOCK" \
    K2_PROJECT_PATH="$WORK" \
    "$K2_CLI" mail messages --json >/dev/null 2>"$WORK/cli.err"
mail_rc=$?
set -e

if [ ! -f "$WORK/last_req.json" ]; then
    echo "  FAIL: stub saw no request (cli rc=$mail_rc err=$(cat "$WORK/cli.err" 2>/dev/null))" >&2
    fail=$((fail + 1))
else
    tok="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("token_query",""))' "$WORK/last_req.json")"
    assert_eq "mail TCP token is scoped (not owner)" "$tok" "$SCOPED"
    if [ "$tok" = "$OWNER" ]; then
        echo "  FAIL: owner token leaked on ID tool path" >&2
        fail=$((fail + 1))
    fi
fi

# Exit-3 path end-to-end (no SCOPED token in env)
set +e
env -u K2SO_HOOK_SOCK -u K2SO_HOOK_TOKEN -u K2_HOOK_TOKEN -u K2SO_PORT \
    K2_PORT="$STUB_PORT" \
    K2_HOOK_SOCK="$FAKE_SOCK" \
    K2_PROJECT_PATH="$WORK" \
    "$K2_CLI" mail messages --json >/dev/null 2>"$WORK/cli_no_passport.err"
e3_rc=$?
set -e
assert_eq "e2e ID+sock+no-passport exit 3" "$e3_rc" "3"
if grep -q "session ID (passport)" "$WORK/cli_no_passport.err"; then
    echo "  PASS: e2e teaching error on stderr"
    pass=$((pass + 1))
else
    echo "  FAIL: e2e missing passport teaching error: $(cat "$WORK/cli_no_passport.err")" >&2
    fail=$((fail + 1))
fi

echo
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
