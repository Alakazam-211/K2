#!/usr/bin/env bash
# k2 desktop on a Linux server with no screen (prd-linux-virtual-desktop-v1
# L4): start / stop / setup / list, --desktop and --size, and what status
# prints for a virtual desktop. A stub daemon answers with the wire
# contract's shapes and logs each request (path, query, body).
# Hermetic: HOME is a temp dir; no real daemon, no ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

WORK="$(mktemp -d -t k2-vd-cli-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT
mkdir -p "$WORK/home"

python3 -I -c '
import json, os, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
STATUS = {
    "desktop": "virtual:ws1", "kind": "agent", "name": "scientist", "workspaceId": "ws1",
    "machine": "your-server", "enabled": True, "backend": "linux", "lane": "main",
    "state": "ready", "message": "Ready.", "reason": None,
    "you": {"role": "owner", "mayView": True, "mayControl": True, "mayManage": True, "sameMachine": False, "mayStart": True},
    "virtual": {
        "size": {"width": 1600, "height": 900}, "display": ":100", "memoryBytes": 73400320,
        "present": [{"viewer": "dv_1", "user": "rosson"}],
        "hold": {"by": "k2/scientist", "until": 1731301800},
        "nextSleepAt": None, "nextCloseAt": None, "canSleep": True, "screen": "ready", "waiting": None,
        "label": "Separate, not private",
        "limits": {"desktops": {"max": 4, "used": 2}, "awake": {"max": 2, "used": 1},
                   "memoryBytes": {"max": 2147483648, "each": 2147483648}, "cpuCores": 2},
        "missing": [], "manual": None, "suggestRecycle": False,
    },
}
LIST = {"default": "virtual", "desktops": [
    {"id": "virtual", "kind": "server", "name": "This server", "state": "asleep"},
    {"id": "virtual:ws1", "kind": "agent", "name": "scientist", "workspaceId": "ws1", "state": "ready", "label": "Separate, not private"}]}
class H(BaseHTTPRequestHandler):
    def _reply(self, code, obj):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def _log(self, body):
        u = urllib.parse.urlparse(self.path)
        q = "&".join(p for p in u.query.split("&") if p and not p.startswith("token="))
        with open(os.path.join(work, "requests.log"), "a") as f:
            f.write("%s %s?%s %s\n" % (self.command, u.path, q, body))
        # The credential it came with (query token or Bearer), for the
        # passport checks; never in requests.log.
        sent = [urllib.parse.unquote(p[6:]) for p in u.query.split("&") if p.startswith("token=")]
        sent.append(self.headers.get("Authorization") or "")
        with open(os.path.join(work, "tokens.log"), "a") as f:
            f.write(" ".join(sent) + "\n")
        return u.path
    def do_GET(self):
        path = self._log("")
        if path == "/cli/desktop/list":
            return self._reply(200, LIST)
        polls = os.path.join(work, "setup_polls")
        if os.path.exists(polls):
            n = int(open(polls).read())
            if n > 0:
                open(polls, "w").write(str(n - 1))
                return self._reply(200, dict(STATUS, state="setting_up", message="Installing the desktop packages\u2026"))
            if os.path.exists(os.path.join(work, "setup_fail")):
                return self._reply(200, dict(STATUS, state="not_set_up", message="The desktop packages didn\u0027t install: E: Unable to locate package xvfb. Set up again."))
            return self._reply(200, dict(STATUS, state="closed", message="This server has no screen. K2 can start a desktop on it."))
        return self._reply(200, STATUS)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        path = self._log(body)
        if path == "/cli/desktop/start":
            return self._reply(200, STATUS)
        if path == "/cli/desktop/stop":
            return self._reply(200, {"ok": True, "desktop": "virtual:ws1", "closed": True, "state": "closed", "message": "The desktop was closed by rosson."})
        if path == "/cli/desktop/setup" and os.path.exists(os.path.join(work, "setup_async")):
            open(os.path.join(work, "setup_polls"), "w").write("2")
            return self._reply(200, {"ok": True, "state": "setting_up", "message": "Installing the desktop packages\u2026"})
        if path == "/cli/desktop/env":
            b = json.loads(body or "{}")
            if b.get("release"):
                return self._reply(200, {"ok": True, "desktop": "virtual:ws1", "released": True})
            if os.path.exists(os.path.join(work, "env_off")):
                return self._reply(403, {"ok": False, "error": {"code": "desktop_switch_off", "hint": "the \"Allow a virtual desktop\" switch is off"},
                                         "permission": {"requestable": True, "command": "k2 permission request desktop.use --why WHY"}})
            polls = os.path.join(work, "env_polls")
            ready = {"ok": True, "desktop": "virtual:ws1", "name": "scientist", "state": "ready", "message": "Ready.",
                     "display": ":101", "xauthority": "/home/k2/.k2/desktop/virtual-ws1/Xauthority", "profile": "/home/k2/.k2/desktop/virtual-ws1/profile",
                     "env": {"DISPLAY": ":101", "XAUTHORITY": "/home/k2/.k2/desktop/virtual-ws1/Xauthority", "K2_DESKTOP_PROFILE": "/home/k2/.k2/desktop/virtual-ws1/profile"},
                     "hold": {"by": "scientist", "until": 1731301800}, "holdFor": 1800, "label": "Separate, not private"}
            if os.path.exists(polls):
                n = int(open(polls).read())
                if n > 0:
                    open(polls, "w").write(str(n - 1))
                    return self._reply(200, dict(ready, state="waiting", message="Waiting for another desktop to go to sleep.", display=None, xauthority=None, env={}))
            return self._reply(200, ready)
        if path == "/cli/agent-access/set":
            b = json.loads(body)
            return self._reply(200, {"ok": True, "workspace": "scientist", "toggle": b.get("toggle"), "agentsCanUseDesktop": b.get("value")})
        if path == "/cli/desktop/setup":
            return self._reply(409, {"ok": False, "error": {"code": "setup_manual", "hint": "Run this, then try again: sudo apt-get update && sudo apt-get install --no-install-recommends xvfb icewm fonts-dejavu-core"}, "manual": "sudo apt-get update"})
        return self._reply(404, {"ok": False, "error": {"code": "not_found", "hint": path}})
    def log_message(self, *a):
        pass
s = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(work, "port.tmp"), "w").write(str(s.server_port))
os.rename(os.path.join(work, "port.tmp"), os.path.join(work, "port"))
s.serve_forever()
' "$WORK" &
STUB_PID=$!
for _ in $(seq 1 100); do [ -f "$WORK/port" ] && break; sleep 0.05; done
[ -f "$WORK/port" ] || { echo "FAIL: the stub daemon didn't start" >&2; exit 1; }

k2() {
    env -i PATH="$PATH" HOME="$WORK/home" K2_PORT="$(cat "$WORK/port")" K2_HOOK_TOKEN=test-token "$K2_CLI" desktop "$@"
}

fail=0
pass() { echo "  PASS: $1"; }
bad() { echo "  FAIL: $1" >&2; fail=1; }
last_req() { tail -1 "$WORK/requests.log"; }

out="$(k2 status --desktop virtual:ws1)"
# Percent-escapes differ in case between encoders: compare lower case.
case "$(last_req | tr 'A-Z' 'a-z')" in "get /cli/desktop/status?desktop=virtual%3aws1 ") pass "status sends ?desktop=" ;; *) bad "status request: $(last_req)" ;; esac
for want in "desktop        : virtual:ws1 (scientist, an agent)" "size           : 1600x900" "viewing now    : rosson" \
    "agent hold     : k2/scientist until" "limits         : 2/4 desktops, 1/2 awake, 2048 MB for all (2048 MB each), 2 cores" \
    "note           : Separate, not private" "you may start and close it"; do
    case "$out" in *"$want"*) pass "status: $want" ;; *) bad "status lacks '$want': $out" ;; esac
done

k2 start --desktop virtual:ws1 --size 1600x900 >/dev/null
case "$(last_req)" in *"POST /cli/desktop/start?"*'"width": 1600'*'"height": 900'*'"desktop": "virtual:ws1"'*) pass "start sends the size and the desktop" ;; *) bad "start request: $(last_req)" ;; esac

set +e
k2 start --size big >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = "2" ] && pass "a bad --size is a usage error" || bad "--size big exit $rc"
set +e
k2 status --size 1600x900 >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = "2" ] && pass "--size only goes with start" || bad "status --size exit $rc"

out="$(k2 list)"
case "$out" in *"virtual:ws1"*"scientist"*"Separate, not private"*) pass "list" ;; *) bad "list: $out" ;; esac

out="$(k2 stop --desktop virtual:ws1)"
case "$out" in *"Closed. The desktop was closed by rosson."*) pass "stop" ;; *) bad "stop: $out" ;; esac
case "$(last_req)" in *'"desktop": "virtual:ws1"'*) pass "stop sends the desktop" ;; *) bad "stop request: $(last_req)" ;; esac

set +e
err="$(k2 setup 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" != "0" ] && pass "setup_manual exits non-zero" || bad "setup exit 0"
case "$err" in *"setup_manual"*"sudo apt-get update"*) pass "setup prints the command" ;; *) bad "setup: $err" ;; esac

# Setup answers setting_up at once (contract note 11-11); the CLI waits on
# status until closed (installed) or not_set_up (why), printing progress.
touch "$WORK/setup_async"
set +e
out="$(k2 setup 2>"$WORK/err")"; rc=$?
set -e
[ "$rc" = "0" ] && pass "setup waits and exits 0 when installed" || bad "setup exit $rc: $(cat "$WORK/err")"
case "$out" in *"Installed. k2 desktop start starts it."*) pass "setup says installed" ;; *) bad "setup: $out" ;; esac
case "$(cat "$WORK/err")" in *"Installing the desktop packages"*) pass "setup prints progress" ;; *) bad "setup progress: $(cat "$WORK/err")" ;; esac
case "$(grep -c 'GET /cli/desktop/status?desktop=virtual' "$WORK/requests.log")" in 0) bad "setup never polled status" ;; *) pass "setup polls status for virtual" ;; esac

touch "$WORK/setup_fail"
set +e
err="$(k2 setup 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" = "1" ] && pass "a failed install exits 1" || bad "failed setup exit $rc"
case "$err" in *"setup_failed"*"Unable to locate package xvfb"*) pass "a failed install says why" ;; *) bad "failed setup: $err" ;; esac
rm -f "$WORK/setup_fail"

out="$(k2 setup --no-wait)"
case "$out" in *"k2 desktop status says setting_up"*) pass "--no-wait returns at once" ;; *) bad "--no-wait: $out" ;; esac
set +e
k2 status --no-wait >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = "2" ] && pass "--no-wait only goes with setup" || bad "status --no-wait exit $rc"

# "Allow a virtual desktop" for one agent (LD-38): agent-access/set with
# toggle desktop; no on/off reads it from status by workspace.
out="$(k2 allow on --workspace /ws/scientist)"
case "$(last_req)" in *"POST /cli/agent-access/set?"*'"workspace": "/ws/scientist"'*'"toggle": "desktop"'*'"value": true'*) pass "allow on posts toggle desktop" ;; *) bad "allow on request: $(last_req)" ;; esac
case "$out" in *"Allow a virtual desktop is on for scientist."*"k2 desktop env"*) pass "allow on says so" ;; *) bad "allow on: $out" ;; esac
out="$(k2 allow off --workspace /ws/scientist)"
case "$out" in *"Allow a virtual desktop is off for scientist."*) pass "allow off" ;; *) bad "allow off: $out" ;; esac
case "$(last_req)" in *'"value": false'*) pass "allow off sends false" ;; *) bad "allow off request: $(last_req)" ;; esac
out="$(k2 allow --workspace ws1)"
case "$(last_req)" in "GET /cli/desktop/status?workspace=ws1 ") pass "allow reads status by workspace" ;; *) bad "allow request: $(last_req)" ;; esac
case "$out" in *"Allow a virtual desktop is"*"for scientist."*) pass "allow shows it" ;; *) bad "allow: $out" ;; esac
set +e
k2 status --workspace ws1 >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = "2" ] && pass "--workspace only goes with allow" || bad "status --workspace exit $rc"
set +e
k2 allow maybe >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = "2" ] && pass "allow takes on or off" || bad "allow maybe exit $rc"

# k2 desktop env (L8): shell lines for eval, the hold in a comment.
out="$(k2 env)"
case "$(last_req)" in "POST /cli/desktop/env? {}") pass "env posts {}" ;; *) bad "env request: $(last_req)" ;; esac
case "$out" in *"export DISPLAY=:101"*"export XAUTHORITY=/home/k2/.k2/desktop/virtual-ws1/Xauthority"*"export K2_DESKTOP_PROFILE=/home/k2/.k2/desktop/virtual-ws1/profile"*) pass "env prints exports" ;; *) bad "env: $out" ;; esac
case "$out" in *"# virtual:ws1 (scientist): Ready. Yours until"*"--release when done"*"Separate, not private"*) pass "env says whose and until when" ;; *) bad "env comment: $out" ;; esac
got="$(env -i PATH="$PATH" bash -c "eval \"\$1\"; printf '%s|%s' \"\$DISPLAY\" \"\$K2_DESKTOP_PROFILE\"" _ "$out")"
[ "$got" = ":101|/home/k2/.k2/desktop/virtual-ws1/profile" ] && pass "eval sets the variables" || bad "eval gave: $got"
out="$(k2 env --release)"
case "$(last_req)" in *'"release": true'*) pass "env --release posts release" ;; *) bad "release request: $(last_req)" ;; esac
case "$out" in *"released"*) pass "env --release says so" ;; *) bad "release: $out" ;; esac
echo 1 > "$WORK/env_polls"
set +e
out="$(k2 env 2>"$WORK/err")"; rc=$?
set -e
[ "$rc" = "0" ] && pass "env waits for a waiting desktop" || bad "env waiting exit $rc"
case "$(cat "$WORK/err")" in *"# waiting: Waiting for another desktop"*) pass "env says it waits" ;; *) bad "env wait line: $(cat "$WORK/err")" ;; esac
case "$out" in *"export DISPLAY=:101"*) pass "then prints the exports" ;; *) bad "env after wait: $out" ;; esac
touch "$WORK/env_off"
set +e
err="$(k2 env 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" = "3" ] && pass "switch off exits 3" || bad "env off exit $rc"
case "$err" in *"desktop_switch_off"*"Ask with: k2 permission request desktop.use"*) pass "switch off says how to ask" ;; *) bad "env off: $err" ;; esac
rm -f "$WORK/env_off"

# k2 desktop env has its own locked id tool, `desktop`: inside a K2
# terminal it always sends the session passport, never the owner token.
# The people's verbs stay ungoverned.
(
    eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2_CLI")"
    [ "$(_cli_tool_id_for_verb desktop env)" = "desktop" ] || { echo "  FAIL: desktop env tool id: $(_cli_tool_id_for_verb desktop env)" >&2; exit 1; }
    [ "$(_cli_tool_default_mode desktop)" = "id" ] || { echo "  FAIL: desktop is not an id tool" >&2; exit 1; }
    _cli_tool_is_locked desktop || { echo "  FAIL: desktop is not locked" >&2; exit 1; }
    [ -z "$(_cli_tool_id_for_verb desktop status)" ] || { echo "  FAIL: desktop status got a tool id" >&2; exit 1; }
) && pass "catalog: desktop env is its own locked id tool" || bad "catalog: desktop env tool"
mkdir -p "$WORK/cell/.k2"
printf 'owner-secret-token' > "$WORK/cell/.k2/daemon.token"
printf 'owner-secret-token' > "$WORK/cell/.k2/heartbeat.token"
cell() {
    env -i PATH="$PATH" HOME="$WORK/cell" K2_PORT="$(cat "$WORK/port")" K2_HOOK_SOCK="$WORK/no-such.sock" "$@" "$K2_CLI" desktop env
}
before="$(wc -l < "$WORK/requests.log")"
set +e
err="$(cell 2>&1 >/dev/null)"; rc=$?
set -e
[ "$rc" = "3" ] && pass "in a K2 terminal with no passport env exits 3" || bad "no passport exit $rc: $err"
case "$err" in *"requires session ID"*) pass "and says it needs the passport" ;; *) bad "no passport: $err" ;; esac
[ "$(wc -l < "$WORK/requests.log")" = "$before" ] && pass "and never asks the daemon as the owner" || bad "a request went out: $(last_req)"
: > "$WORK/tokens.log"
out="$(cell K2_HOOK_TOKEN=passport-abc)"
case "$out" in *"export DISPLAY=:101"*) pass "with a passport env works" ;; *) bad "passport env: $out" ;; esac
case "$(cat "$WORK/tokens.log")" in *passport-abc*) pass "env sent the passport" ;; *) bad "env token: $(cat "$WORK/tokens.log")" ;; esac
case "$(cat "$WORK/tokens.log")" in *owner-secret*) bad "env sent the owner token" ;; *) pass "env never sent the owner token" ;; esac
set +e
k2 status --release >/dev/null 2>&1; rc=$?
set -e
[ "$rc" = "2" ] && pass "--release only goes with env" || bad "status --release exit $rc"

[ "$fail" = "0" ] || exit 1
echo "desktop_virtual_cli: ok"
