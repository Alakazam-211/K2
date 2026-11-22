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
# and firewall, each with its own printer; and `k2 permission list`, whose
# actions each carry `command` and `tail` the same way.
#
# A hint that names a command or the product comes with `hintTemplate`
# beside it, `{cli}` and `{product}` where the names go: the CLI prints the
# template with its own names (k2, K2), never the server's (zz, Globex
# Desk). With no usable template the hint prints as sent. Covered through
# `k2 permission request` (the _k2_hint_fill rule every hint printer uses).
#
# An older CLI, from before tail, against a daemon that sends both keys:
# it still prints the daemon's whole command, unchanged. The older CLI is
# a scratch copy of this checkout's bash CLI with the tail rule taken out
# again (the copy is the bash CLI from before the rule, byte for byte).
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
LIST_TAIL = 'permission request compute.use --why "<what you need it for>"'
HINT_T = "ask for that first: {cli} permission request vault.delivery secret=S delivery=file, from outside {product}"

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a):
        pass
    def handle_any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        mode = open(MODE).read().strip()
        if self.path.startswith("/cli/permission/enabled"):
            return self.reply(200, {"enabled": True})
        if self.path.startswith("/cli/permission/request"):
            e = {"code": "bad_params", "hint": HINT_T.replace("{cli}", "zz").replace("{product}", "Globex Desk")}
            if mode == "new":
                e["hintTemplate"] = HINT_T
            elif mode == "number":
                e["hintTemplate"] = 7
            elif mode == "empty":
                e["hintTemplate"] = ""
            return self.reply(400, {"ok": False, "error": e})
        if self.path.startswith("/cli/permission/list"):
            a = {"id": "compute.use", "command": "zz " + LIST_TAIL}
            if mode == "new":
                a["tail"] = LIST_TAIL
            elif mode == "number":
                a["tail"] = 7
            elif mode == "empty":
                a["tail"] = ""
            return self.reply(200, {"agent": "ws", "actions": [a], "requests": []})
        p = {"action": "connections.add", "params": {"to": "a b'c"}, "approvers": "owner or admin",
             "command": "zz " + TAIL}
        if mode == "new":
            p["tail"] = TAIL
        elif mode == "number":
            p["tail"] = 7
        elif mode == "empty":
            p["tail"] = ""
        self.reply(403, {"ok": False, "error": {"code": "owner_only", "hint": "only the owner may do this"},
                         "permission": p})
    def reply(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
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

# run <args…>: as an agent in a K2 terminal, with $PROG (default the CLI
# under test). Sets ERR and RC.
run() {
    set +e
    K2_PORT="$PORT" K2_HOOK_TOKEN=passport-tail "${PROG:-$K2}" "$@" </dev/null >"$SANDBOX/out" 2>"$SANDBOX/err"
    RC=$?
    set -e
    ERR="$(cat "$SANDBOX/err")"
}

last_err_line() { printf '%s\n' "$ERR" | sed -e '/^$/d' | tail -1; }

# k2 permission list: the action's command, as json.dumps writes it.
LIST_OURS='"command": "k2 permission request compute.use --why \"<what you need it for>\""'
LIST_THEIRS='"command": "zz permission request compute.use --why \"<what you need it for>\""'
list_check() { # <label> <want>
    run permission list
    [ "$RC" = 0 ] && ok || bad "$1: k2 permission list: exit $RC ($ERR)"
    grep -qF -- "$2" "$SANDBOX/out" && ok || bad "$1: k2 permission list: no $2 in: $(cat "$SANDBOX/out")"
    [ "$(grep -c '"command": ' "$SANDBOX/out")" = 1 ] && ok || bad "$1: k2 permission list: one command: $(cat "$SANDBOX/out")"
}

# A hint: the template with this CLI's names, or the hint as sent.
HINT_OURS="Error [bad_params]: ask for that first: k2 permission request vault.delivery secret=S delivery=file, from outside K2"
HINT_THEIRS="Error [bad_params]: ask for that first: zz permission request vault.delivery secret=S delivery=file, from outside Globex Desk"
hint_check() { # <label> <want>
    run permission request vault.delivery secret=S delivery=file --why "for the deploy"
    [ "$RC" = 2 ] && ok || bad "$1: k2 permission request: exit $RC, want 2 ($ERR)"
    [ "$(last_err_line)" = "$2" ] && ok || bad "$1: k2 permission request: the last line: $(last_err_line)"
    [ "$(printf '%s\n' "$ERR" | grep -c '^Error \[')" = 1 ] && ok || bad "$1: k2 permission request: one error line: $ERR"
}

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
    if [ "$mode" = new ]; then list_check "$mode" "$LIST_OURS"; else list_check "$mode" "$LIST_THEIRS"; fi
    if [ "$mode" = new ]; then hint_check "$mode" "$HINT_OURS"; else hint_check "$mode" "$HINT_THEIRS"; fi
done

# The older CLI: the bash CLI with the tail rule taken out again.
cat > "$SANDBOX/untail.py" <<'UNTAIL'
import re, sys
s = open(sys.argv[1]).read()
n0 = s.count("<tail>")
# A rule after an assignment: "; v = dict(v, command=...) if ... else v".
s, a = re.subn(r"""; (\w+) = dict\(\1, command=(["'])k2 <tail>\2\.replace\(\2<tail>\2, \1\[\2tail\2\]\)\) if isinstance\(\1, dict\) and isinstance\(\1\.get\(\2tail\2\), str\) and \1\[\2tail\2\] else \1\n""", "\n", s)
# The rule on a line of its own, in place of a comment.
s, b = re.subn(r"""(\n +)p = dict\(p, command="k2 <tail>"\.replace\("<tail>", p\["tail"\]\)\) if isinstance\(p, dict\) and isinstance\(p\.get\("tail"\), str\) and p\["tail"\] else p  # the rules of _permission_closing_lines\n""", r"\1# Same rules as _permission_closing_lines.\n", s)
# mail's die: cmd from the tail.
s, c = re.subn(r"""cmd = "k2 <tail>"\.replace\("<tail>", permission\["tail"\]\) if isinstance\(permission\.get\("tail"\), str\) and permission\["tail"\] else permission\.get\("command"\)""", 'cmd = permission.get("command")', s)
# permission list: each action's command from its tail.
s, d = re.subn(r"""\[a\.update\(command="k2 <tail>"\.replace\("<tail>", a\["tail"\]\)\) for a in \(v\.get\("actions"\) if verb == "list" and isinstance\(v\.get\("actions"\), list\) else \[\]\) if isinstance\(a, dict\) and isinstance\(a\.get\("tail"\), str\) and a\["tail"\]\]; """, "", s)
# An ask from its askTail (a test login's rooms, an invite's rooms, a
# vault delivery with no card yet, a widget's review).
s, e = re.subn(r"""\("k2 <tail>"\.replace\("<tail>", (\w+)\["askTail"\]\) if isinstance\(\1\.get\("askTail"\), str\) and \1\["askTail"\] else \1\.get\("ask"\)\)""", r'\1.get("ask")', s)
# The hint rule: k2_hint_fill leaves every hint as sent.
s, f = re.subn(r"""\n( +)v\["hint"\] = t\.replace\("\{cli\}", _names\[0\]\)\.replace\("\{product\}", _names\[1\]\)\n""", r"\n\1pass\n", s)
if "<tail>" in s or a + b + c == 0 or d != 1 or e != 7 or f != 1:
    sys.exit("untail: the tail rule is still there (%d left of %d; %d %d %d %d %d %d)" % (s.count("<tail>"), n0, a, b, c, d, e, f))
open(sys.argv[2], "w").write(s)
print("untail: %d rules taken out" % (a + b + c + d + e + f))
UNTAIL
OLD="$SANDBOX/old-k2"
python3 -I "$SANDBOX/untail.py" "$CLI_BASH_SOURCE" "$OLD" >/dev/null
chmod +x "$OLD"
if grep -q '<tail>' "$OLD" || ! grep -q 'p = v.get("permission") if isinstance(v, dict) else None$' "$OLD"; then
    bad "the older CLI still has the tail rule, or lost its printer"
fi
echo new > "$MODE"
for form in "connections add billing" "desktop env" "front status" "firewall"; do
    # shellcheck disable=SC2086
    PROG="$OLD" run $form
    [ "$RC" != 0 ] && ok || bad "older CLI: k2 $form: exit 0 on a refusal ($ERR)"
    [ "$(last_err_line)" = "$THEIRS" ] && ok || bad "older CLI: k2 $form: the last line: $(last_err_line)"
    [ "$(printf '%s\n' "$ERR" | grep -c '^Ask with: ')" = 1 ] && ok || bad "older CLI: k2 $form: one Ask line: $ERR"
done
PROG="$OLD" list_check "older CLI" "$LIST_THEIRS"
PROG="$OLD" hint_check "older CLI" "$HINT_THEIRS"

echo "permission_ask_tail: $pass passed, $fail failed"
[ "$fail" = 0 ]
