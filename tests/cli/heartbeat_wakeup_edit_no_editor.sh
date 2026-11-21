#!/usr/bin/env bash
# `k2 heartbeat wakeup <name> --edit` with EDITOR unset opens vi, and its
# note names vi too.
#
# Before the fix the note read $EDITOR under `set -u`: with EDITOR unset
# and no wakeup file yet, the command stopped on "EDITOR: unbound
# variable" (exit 1) before it opened anything. The exec already used
# ${EDITOR:-vi}.
#
# A stub daemon on loopback answers /cli/heartbeat/list with one heartbeat
# whose wakeup file does not exist. `vi` (and a named editor) are stand-ins
# on PATH that log their arguments and exit 0, so nothing real opens.
#
# Hermetic: HOME is a throwaway dir, every K2_*/K2SO_*/K5_* var is unset,
# no real daemon. Run with: bash tests/cli/heartbeat_wakeup_edit_no_editor.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

for v in $(env | grep -E '^K[25](SO)?_' | cut -d= -f1); do unset "$v"; done
unset EDITOR VISUAL

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1"; diff <(printf '%s\n' "$3") <(printf '%s\n' "$2") >&2 || true; fi; }

SANDBOX="$(mktemp -d /tmp/k2hbwe-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then kill "$STUB_PID" 2>/dev/null || true; wait "$STUB_PID" 2>/dev/null || true; fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
ROWS = [{"name": "nightly", "wakeupPath": ".k2/heartbeats/nightly/wakeup.md"}]

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        st, obj = (200, ROWS) if path == "/cli/heartbeat/list" else (404, {"error": {"code": "not_found", "hint": "no such route"}})
        body = json.dumps(obj).encode()
        self.send_response(st)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    do_POST = do_GET

srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$SANDBOX/stub.py" "$SANDBOX" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"
export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws" "$SANDBOX/bin"
for ed in vi myedit; do
    printf '#!/bin/sh\necho "%s $*" >> "%s/editor.log"\n' "$ed" "$SANDBOX" > "$SANDBOX/bin/$ed"
    chmod +x "$SANDBOX/bin/$ed"
done
cd "$SANDBOX/ws"
WAKEUP="$SANDBOX/ws/.k2/heartbeats/nightly/wakeup.md"

# rc_of [VAR=value…] -- <args…>: an agent in a K2 terminal over TCP.
rc_of() {
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    : >"$SANDBOX/editor.log"
    local rc=0
    env ${envs[@]+"${envs[@]}"} PATH="$SANDBOX/bin:$PATH" K2_PORT="$PORT" K2_HOOK_TOKEN=passport-hbwe \
        "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err" || rc=$?
    echo "$rc"
}

echo "== EDITOR unset, no wakeup file: vi, named in the note =="
rc="$(rc_of -- heartbeat wakeup nightly --edit)"
expect_eq "EDITOR unset: exit 0" "$rc" "0"
expect_eq "EDITOR unset: the note names vi" "$(cat "$SANDBOX/out")" \
"Note: $WAKEUP does not exist yet; vi will create it."
expect_eq "EDITOR unset: nothing on stderr" "$(cat "$SANDBOX/err")" ""
expect_eq "EDITOR unset: vi opens the wakeup file" "$(cat "$SANDBOX/editor.log")" "vi $WAKEUP"

echo "== EDITOR set, no wakeup file: that editor, named in the note =="
rc="$(rc_of EDITOR=myedit -- heartbeat wakeup nightly --edit)"
expect_eq "EDITOR=myedit: exit 0" "$rc" "0"
expect_eq "EDITOR=myedit: the note names it" "$(cat "$SANDBOX/out")" \
"Note: $WAKEUP does not exist yet; myedit will create it."
expect_eq "EDITOR=myedit: it opens the wakeup file" "$(cat "$SANDBOX/editor.log")" "myedit $WAKEUP"

echo
echo "heartbeat_wakeup_edit_no_editor: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
