#!/usr/bin/env bash
# The CLI's embedded Python follows no redirect. Its requests carry a token
# (some a password too), and urllib would send the Authorization header and
# the body on to wherever a 3xx points. _K2_PY_PROXY_RULE's opener refuses
# a 3xx instead: a plain line, exit 3, and nothing sent to the place it
# names.
#
# The real CLI runs against a daemon stub that answers every request with
# a 302 to a second stub. Every python3 the CLI starts loads an audit hook
# (sitecustomize on PYTHONPATH) that logs each connect, so the second stub
# can be shown to get no request and no connection, https included.
#
# Covered: k2 calendar list, k2 project list, k2 mail domains (three of the
# blocks that run the rule; k2 clone has its own rows in clone_security.sh).
#
# Run on compute (it starts stubs and opens ports):
#   bash tests/cli/py_no_redirect.sh
# No daemon build; a throwaway HOME; never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^(K2|K2SO|K5)_' | cut -d= -f1); do unset "$v"; done
unset PYTHONPATH

PY="$(command -v python3)" || { echo "FAIL: python3 not found" >&2; exit 1; }

W="$(mktemp -d /tmp/k2nr-XXXXXX)"
PIDS=()
cleanup() {
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
    rm -rf "$W"
}
trap cleanup EXIT

export HOME="$W/home"
mkdir -p "$HOME/.k2"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if <<<"$2" grep -Fq -- "$3"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

# stub.py NAME PORTFILE: logs each request (method, path, Authorization) to
# $W/log-NAME.jsonl. With $W/location-NAME present, answers every request
# with a 302 to the URL in it; else 200 {"ok": true}.
cat >"$W/stub.py" <<'PYEOF'
import http.server, json, os, sys, threading
name, port_file = sys.argv[1], sys.argv[2]
W = os.path.dirname(os.path.abspath(__file__))
lock = threading.Lock()
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        with lock, open(os.path.join(W, "log-%s.jsonl" % name), "a") as f:
            f.write(json.dumps({"method": self.command, "path": self.path.split("?")[0],
                                "auth": self.headers.get("Authorization", "")}) + "\n")
        loc_file = os.path.join(W, "location-%s" % name)
        data = json.dumps({"ok": True}).encode()
        if os.path.exists(loc_file):
            self.send_response(302)
            self.send_header("Location", open(loc_file).read().strip())
        else:
            self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(port_file + ".tmp", "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(port_file + ".tmp", port_file)
srv.serve_forever()
PYEOF
"$PY" "$W/stub.py" daemon "$W/port-daemon" & PIDS+=("$!")
"$PY" "$W/stub.py" other "$W/port-other" & PIDS+=("$!")
for _ in $(seq 1 100); do
    [ -s "$W/port-daemon" ] && [ -s "$W/port-other" ] && break
    sleep 0.1
done
[ -s "$W/port-daemon" ] && [ -s "$W/port-other" ] || { echo "FAIL: a stub never came up" >&2; exit 1; }
PD="$(cat "$W/port-daemon")"; PO="$(cat "$W/port-other")"

TOKEN="k2ot_NOREDIRECT_owner_token"
printf '%s\n' "$PD" >"$HOME/.k2/heartbeat.port"
printf '%s\n' "$TOKEN" >"$HOME/.k2/heartbeat.token"
printf '%s\n' "$TOKEN" >"$HOME/.k2/daemon.token"

mkdir -p "$W/audit"
cat >"$W/audit/sitecustomize.py" <<'PYEOF'
import os, sys
_log = os.environ["K2T_AUDIT_LOG"]
def _hook(ev, args):
    if ev == "socket.connect":
        with open(_log, "a") as f:
            f.write("%s %r\n" % (ev, args[1:2]))
sys.addaudithook(_hook)
PYEOF
export PYTHONPATH="$W/audit"
export K2T_AUDIT_LOG="$W/audit.log"

# daemon_paths / other_paths: what each stub got, one "METHOD path" a line.
paths() { [ -f "$W/log-$1.jsonl" ] || return 0; "$PY" -c '
import json, sys
for l in open(sys.argv[1]):
    r = json.loads(l); print(r["method"], r["path"])' "$W/log-$1.jsonl"; }
auths() { [ -f "$W/log-$1.jsonl" ] || return 0; "$PY" -c '
import json, sys
for l in open(sys.argv[1]):
    print(json.loads(l)["auth"])' "$W/log-$1.jsonl"; }

echo "== no request follows a redirect =="
for to in "http://127.0.0.1:$PO/elsewhere" "https://127.0.0.1:$PO/other-host"; do
    printf '%s\n' "$to" >"$W/location-daemon"
    for verb in "calendar list --json" "project list --json" "mail domains --json"; do
        rm -f "$W"/log-*.jsonl; : >"$W/audit.log"
        set +e
        # shellcheck disable=SC2086
        "$K2" $verb </dev/null >"$W/out" 2>"$W/err"; rc=$?
        set -e
        err="$(cat "$W/err")"
        label="k2 $verb, 302 to $to"
        expect_eq "$label: exit 3" "$rc" "3"
        expect_has "$label: the refusal line" "$err" "refused: 127.0.0.1:$PD answered 302, sending K2 to 127.0.0.1:$PO; K2 does not follow a redirect with your token or password, so nothing was sent there"
        expect_has "$label: the daemon got the token" "$(auths daemon)" "Bearer $TOKEN"
        expect_eq "$label: the place it named got nothing" "$(paths other)" ""
        expect_not "$label: no connect to it" "$(cat "$W/audit.log")" ", $PO)"
    done
done

echo
echo "py_no_redirect: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
