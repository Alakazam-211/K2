#!/usr/bin/env bash
# The Ask line takes this CLI's own command name (white-label step C5).
#
# A refusal's `permission` object carries `command` (the whole line, with
# the server's command name) and, from a newer daemon, `tail` (the line
# after the name, already quoted). The CLI prints "Ask with: <its own
# compiled name> <tail>", so a CLI of one brand talking to a server of
# another prints a command that exists on its own machine. With no usable
# `tail` (an older daemon, or not a non-empty string) it prints `command`
# as the daemon sent it, as before.
#
# The stub daemon puts its own command name in `command`: `zz`, a name no
# brand of ours has, so the test means the same under any brand's build. A
# line that still printed `command` would show it. Covered: the shared
# transports' closing line (connections add), desktop env, front status
# and firewall, each with its own printer.
#
# Hermetic: throwaway HOME, every K2_*/K2SO_* var unset, a python stub
# daemon on loopback. No real daemon, ~/.k2 or keychain.
#
#   bash tests/cli/permission_ask_tail.sh      (fails loudly on any miss)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

SANDBOX="$(mktemp -d /tmp/k2pat-XXXXXX)"
SANDBOX="$(cd "$SANDBOX" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$SANDBOX"
}
trap cleanup EXIT

# The stub: every route answers 403 with a permission object; MODE picks it.
#   new     tail and a command of another brand
#   old     command only (a daemon from before tail)
#   number  tail is not a string
#   empty   tail is ""
cat > "$SANDBOX/stub.py" <<'PY'
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

MODE = sys.argv[2]
TAIL = 'permission request connections.add to=\'a b\'\\\'\'c\' --why "<what you need it for>"'

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        mode = open(MODE).read().strip()
        p = {"action": "connections.add", "params": {"to": "a b'c"}, "approvers": "owner or admin",
             "command": "zz " + TAIL}
        if mode == "new":
            p["tail"] = TAIL
        elif mode == "number":
            p["tail"] = 7
        elif mode == "empty":
            p["tail"] = ""
        body = json.dumps({"ok": False, "error": {"code": "owner_only", "hint": "only the owner may do this"},
                           "permission": p}).encode()
        self.send_response(403)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    do_GET = handle_any
    do_POST = handle_any
    do_PUT = handle_any
    do_DELETE = handle_any

srv = HTTPServer(("127.0.0.1", 0), H)
with open(sys.argv[1], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

MODE="$SANDBOX/mode"
echo new > "$MODE"
python3 -I "$SANDBOX/stub.py" "$SANDBOX/port" "$MODE" &
STUB_PID=$!
for _ in $(seq 1 50); do [ -s "$SANDBOX/port" ] && break; sleep 0.1; done
[ -s "$SANDBOX/port" ] || { echo "FAIL: the stub daemon did not start" >&2; exit 1; }
PORT="$(cat "$SANDBOX/port")"

export HOME="$SANDBOX/home"
mkdir -p "$HOME" "$SANDBOX/ws"
cd "$SANDBOX/ws"

pass=0
fail=0
ok() { pass=$((pass + 1)); }
bad() { echo "  FAIL: $*" >&2; fail=$((fail + 1)); }

# run <args…>: as an agent in a K2 terminal. Sets ERR and RC.
run() {
    set +e
    K2_PORT="$PORT" K2_HOOK_TOKEN=passport-tail "$K2" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
    RC=$?
    set -e
    ERR="$(cat "$SANDBOX/err")"
}

last_err_line() { printf '%s\n' "$ERR" | sed -e '/^$/d' | tail -1; }

OURS="Ask with: k2 permission request connections.add to='a b'\\''c' --why \"<what you need it for>\""
THEIRS="Ask with: zz permission request connections.add to='a b'\\''c' --why \"<what you need it for>\""

for mode in new old number empty; do
    echo "$mode" > "$MODE"
    if [ "$mode" = new ]; then want="$OURS"; else want="$THEIRS"; fi
    for form in "connections add billing" "desktop env" "front status" "firewall"; do
        # shellcheck disable=SC2086
        run $form
        [ "$RC" != 0 ] && ok || bad "$mode: k2 $form: exit 0 on a refusal ($ERR)"
        [ "$(last_err_line)" = "$want" ] && ok || bad "$mode: k2 $form: the last line: $(last_err_line)"
        [ "$(printf '%s\n' "$ERR" | grep -c '^Ask with: ')" = 1 ] && ok || bad "$mode: k2 $form: one Ask line: $ERR"
        if [ "$mode" = new ]; then
            case "$ERR" in *"zz permission"*) bad "$mode: k2 $form: the server's name leaked: $ERR" ;; *) ok ;; esac
        fi
    done
done

echo "permission_ask_tail: $pass passed, $fail failed"
[ "$fail" = 0 ]
