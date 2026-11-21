#!/usr/bin/env bash
# A token read with --token-stdin from a person at a terminal: `k2 fed
# pair-join --token-stdin` (and `-`) and `k2 tunnel switch <sub>
# --token-stdin` ask for it on the terminal ("Token: ", "Tunnel token: "),
# read it with echo off (the terminal never shows it), put echo back, and
# send it to the daemon as typed (decoded, for a pairing token).
#
# The pty driver (tests/cli/clone_security.sh's) plays the person: stdin
# and stderr are a pty that is the session's controlling terminal. A stub
# daemon logs each request with its body. No real daemon, no real home.
#
# native: fed pair-join
# native: tunnel switch
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
unset K2_HOOK_TOKEN

PY="$(command -v python3)"
W="$(cd "$(mktemp -d -t k2-token-tty-XXXXXX)" && pwd -P)"
STUB_PID=""
cleanup() {
    if [ -n "$STUB_PID" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$W"
    hermetic_cli_cleanup
}
trap cleanup EXIT
pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

"$PY" - "$W" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
BODY = json.dumps({
    "ok": True, "offer": {"n": 1}, "answer": {"n": 2}, "reveal": {"n": 3},
    "sas": "1234", "peer_fingerprint": "ab" * 32, "subdomain": "acme",
}).encode()
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _go(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, self.path.split("?")[0], body))
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(BODY)))
        self.end_headers()
        self.wfile.write(BODY)
    do_GET = do_POST = _go
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$W/port" ] && break
    sleep 0.1
done
[ -s "$W/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
export K2_PORT="$(cat "$W/port")"
mkdir -p "$HOME/.k2" "$W/proj/.k2"
printf 'owner-tok-tty\n' >"$HOME/.k2/daemon.token"
printf 'owner-tok-tty\n' >"$HOME/.k2/heartbeat.token"
export K2_PROJECT_PATH="$W/proj"
cd "$W/proj"

cat >"$W/ptyrun.py" <<'PYEOF'
import fcntl, json, os, select, signal, subprocess, sys, termios, time
mode, steps, W = sys.argv[1], json.loads(sys.argv[2]), sys.argv[3]
cmd = sys.argv[sys.argv.index("--") + 1:]
if mode == "noctty":
    cmd = [sys.executable, "-c", """
import os, sys
pid = os.fork()
if pid == 0:
    os.execvp(sys.argv[1], sys.argv[1:])
_, st = os.waitpid(pid, 0)
sys.exit(os.waitstatus_to_exitcode(st) if os.WIFEXITED(st) else 128 + os.WTERMSIG(st))
"""] + cmd
m, s = os.openpty()
def pre():
    os.setsid()
    if mode == "ctty":
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
with open(os.path.join(W, "out"), "wb") as out:
    p = subprocess.Popen(cmd, stdin=s, stdout=out, stderr=s, preexec_fn=pre)
def stop():
    try:
        os.killpg(p.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
seen = b""
def pump(t):
    global seen
    r, _, _ = select.select([m], [], [], t)
    if r:
        seen += os.read(m, 65536)
for want, send in steps:
    end = time.time() + 60
    while want.encode() not in seen:
        if time.time() > end or p.poll() is not None:
            pump(0.2)
            if want.encode() in seen:
                break
            open(os.path.join(W, "tty"), "wb").write(seen)
            stop()
            sys.exit("ptyrun: never saw %r on the terminal" % want)
        pump(0.1)
    seen = seen.replace(want.encode(), b"<<" + want.encode() + b">>", 1)
    os.write(m, send.encode())
end = time.time() + 120
while p.poll() is None:
    if time.time() > end:
        stop()
        sys.exit("ptyrun: the command never finished")
    pump(0.1)
pump(0.2)
open(os.path.join(W, "tty"), "wb").write(seen)
# The session's end hangs up the slave descriptor; on Linux the master
# reads the terminal's settings as the command left them.
with open(os.path.join(W, "echo"), "w") as f:
    f.write("on" if termios.tcgetattr(m)[3] & termios.ECHO else "off")
sys.exit(p.returncode if p.returncode >= 0 else 128 - p.returncode)
PYEOF

# k2pty STEPS ARGS…: the command at a terminal; sets RC, TTY (what the
# terminal showed, CRs dropped), ECHO (on or off after the run) and DRV
# (the driver's own complaint, if any).
k2pty() {
    local steps="$1"; shift
    : >"$W/requests.log"
    set +e
    "$PY" "$W/ptyrun.py" ctty "$steps" "$W" -- "$K2" "$@" 2>"$W/drv"; RC=$?
    set -e
    TTY="$(tr -d '\r' <"$W/tty")"; ECHO="$(cat "$W/echo" 2>/dev/null || echo none)"
    DRV="$(cat "$W/drv")"
}

check() {
    local label="$1" prompt="$2" secret="$3" route="$4" sent="$5"
    case "$DRV" in *ptyrun:*) bad "$label: $DRV"; return ;; esac
    case "$TTY" in *"<<$prompt>>"*) ok "$label: asks on the terminal" ;; *) bad "$label: no '$prompt' prompt: $TTY" ;; esac
    case "$TTY" in *"$secret"*) bad "$label: the terminal showed the token: $TTY" ;; *) ok "$label: the token is not echoed" ;; esac
    [ "$ECHO" = on ] && ok "$label: echo is back on" || bad "$label: echo is $ECHO after the run"
    if grep -F "POST $route " "$W/requests.log" | grep -qF "$sent"; then
        ok "$label: the token reached $route"
    else
        bad "$label: $route never got the token:"; sed 's/^/      | /' "$W/requests.log" >&2
    fi
}

# 1. tunnel switch: "Tunnel token: ", the token typed, sent in the body.
k2pty '[["Tunnel token: ", "tun-TTY-7f3a\n"]]' tunnel switch acme --token-stdin
check "tunnel switch --token-stdin" "Tunnel token: " "tun-TTY-7f3a" /cli/tunnel/switch '"token": "tun-TTY-7f3a"'

# 2. fed pair-join --token-stdin and -: "Token: ", the pairing token typed,
#    its offer decoded into the request.
SENT="pair-TTY-91c2"
FED_TOKEN="k2pair1:$("$PY" -c 'import base64,json,sys; print(base64.urlsafe_b64encode(json.dumps({"n": sys.argv[1]}).encode()).decode().rstrip("="))' "$SENT")"
for form in --token-stdin -; do
    k2pty "[[\"Token: \", \"$FED_TOKEN\\n\"]]" fed pair-join "$form"
    check "fed pair-join $form" "Token: " "$FED_TOKEN" /cli/federation/pair/v2/respond "$SENT"
done

echo "token_stdin_tty: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
