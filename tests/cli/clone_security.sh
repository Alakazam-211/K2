#!/usr/bin/env bash
# k2 clone: where its tokens, passwords and files may go.
#
# The real CLI runs against stubs: this server's daemon (TCP, loopback),
# a remote server (TCP, loopback) and the vault socket. Every stub logs
# each request it gets (method, path, headers, body). Every python3 the
# CLI starts loads an audit hook (sitecustomize on PYTHONPATH) that logs
# each name lookup and connect, so a refused host can be shown to get no
# request and no lookup. A small pty driver plays the person at a
# terminal (or a process with no terminal at all).
#
# Rows, by rule:
#   the X-K5-* hand-over headers go to this computer's daemon only;
#   a token goes to an https server, or over plain http only to a loopback
#   address as written (127.0.0.0/8, ::1, localhost); a refused host gets
#   no request and no name lookup; a password never goes over plain http;
#   this computer's confirm goes through the vault socket only (none: no
#   prompt, no TCP); a password is read from a real terminal only, with
#   echo off (no controlling terminal: confirm_needs_a_person, never stdin);
#   Ctrl-C stops with 130 and the cancelled line, echo back on; clone-tmp
#   is a folder of ours at 0700 (a link there is refused); the download's
#   .part is 0600 and is gone after a refused range, a short download,
#   Ctrl-C or SIGTERM; the remote token can come on stdin (--token-stdin),
#   and --token T still works with a note that every account sees it;
#   clone approve sends nothing to an agent-proposed host before the
#   person reads the host in full and says yes at the terminal, and then
#   only the token saved for that host (never $K2_REMOTE_TOKEN or --token);
#   no request follows a redirect (the place it names gets nothing); an
#   address with a name before an @ (user info) is refused whatever its
#   scheme, before any lookup, and the refusal never shows that name; a
#   token may also go over plain http to a Tailscale address as written
#   (100.64.0.0/10, fd7a:115c:a1e0::/48), never to a *.ts.net name, and a
#   password still never goes over plain http; this server's own side
#   (K2_HOST over plain http) warns once per command, except for a
#   loopback or Tailscale address or a *.ts.net name as written, an
#   https:// K2_HOST (not plain http), or with K2_PLAIN_HTTP_OK=1; the help says the confirms ask whoever is at the
#   terminal and are no boundary against programs run as the same user.
#
# Run on compute (it starts stubs and opens ports):
#   bash tests/cli/clone_security.sh
# No daemon build; a throwaway HOME; never touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"

for v in $(env | grep -E '^(K2|K2SO|K5)_' | cut -d= -f1); do unset "$v"; done
unset PYTHONPATH

PY="$(command -v python3)" || { echo "FAIL: python3 not found" >&2; exit 1; }

W="$(mktemp -d /tmp/k2cs-XXXXXX)"
PIDS=()
cleanup() {
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
    rm -rf "$W"
}
trap cleanup EXIT

export HOME="$W/home"
KH="$HOME/.k2"
mkdir -p "$KH"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi; }
expect_has() { if <<<"$2" grep -Fq -- "$3"; then ok "$1"; else bad "$1 (missing $(printf %q "$3") in $(printf %q "$2"))"; fi; }
expect_not() { if <<<"$2" grep -Fq -- "$3"; then bad "$1 (found $(printf %q "$3"))"; else ok "$1"; fi; }

# ── The stubs ────────────────────────────────────────────────────────
# stub.py NAME tcp PORTFILE | NAME unix SOCKPATH. Answers from a table of
# defaults; $W/scn-NAME.json (read on every request) overrides a route
# ({"routes": {path: {"status": n, "body": {...}, "location": url}}}) and
# sets the knobs:
#   methods          what confirm/start offers (vault socket and TCP)
#   confirm_methods  what the clone preview offers for a carry confirm
#   rows             the preview's secret rows
#   target           the clone proposal's target
#   rr               read-range: ok, short, fail2 (the second range is
#                    refused), hang2 (the second range hangs; $W/hanging
#                    marks it)
cat >"$W/stub.py" <<'PYEOF'
import base64, http.server, json, os, socketserver, sys, threading, time
name, kind, addr = sys.argv[1], sys.argv[2], sys.argv[3]
W = os.path.dirname(os.path.abspath(__file__))
LOG = os.path.join(W, "log-%s.jsonl" % name)
lock = threading.Lock()
DATA = b"bundle-bytes-" * 100

def scn():
    try:
        with open(os.path.join(W, "scn-%s.json" % name)) as f:
            return json.load(f)
    except FileNotFoundError:
        return {}

def answer(path, q, body, s):
    rows = s.get("rows", [{"name": "API_KEY", "kind": "api_key", "copyable": True, "default": "skip"}])
    preview = {"workspace": {"path": "/w/ws1", "handle": "ws1"}, "history": {"files": 1, "sessions": 1},
               "threads": {}, "envFiles": [], "callerCanCopySecrets": True,
               "secrets": {"available": True, "rows": rows,
                           "confirm": {"methods": s.get("confirm_methods", ["connect-password"])}}}
    if path in s.get("routes", {}):
        r = s["routes"][path]
        return r.get("status", 200), r.get("body", {})
    if path.endswith("/cli/vault/confirm/start"):
        return 200, {"methods": s.get("methods", ["vault-password"]), "expiresIn": 60}
    if path.endswith("/cli/vault/confirm/finish") or path == "/cli/clone/vault/confirm":
        return 200, {"token": "VT-1", "approveToken": "AT-1", "expiresIn": 60}
    if path == "/cli/vault/recipient":
        return 200, {"recipient": "age1stub", "vaultId": "vault-1"}
    if path == "/cli/clone/preview":
        return 200, preview
    if path == "/cli/clone/bundle":
        return 200, {"bundle_path": os.path.join(W, "bundle.tar")}
    if path == "/cli/clone/unpack":
        return 200, {"dest_path": "/dest/ws1", "project": {"id": "p1", "name": "ws1", "path": "/dest/ws1"}}
    if path == "/cli/fs/info":
        return 200, {"home": "/rhome", "separator": "/"}
    if path == "/cli/fs/upload-chunk":
        return 200, ({"path": "/rhome/.k2/clone-tmp/" + body.get("filename", "")} if body.get("is_last") else {"ok": True})
    if path == "/cli/clone/pack":
        return 200, {"job_id": "job-1"}
    if path == "/cli/clone/pack-status":
        return 200, {"phase": "done", "bundle_path": "/rb/bundle-job-1.tar", "size_bytes": len(DATA)}
    if path == "/cli/clone/pack-cleanup":
        return 200, {"ok": True}
    if path == "/cli/fs/read-range":
        off, half, mode = int(q.get("offset", "0")), len(DATA) // 2, s.get("rr", "ok")
        if mode == "ok":
            return 200, {"base64": base64.b64encode(DATA[off:]).decode(), "eof": True}
        if mode == "short":
            return 200, {"base64": base64.b64encode(DATA[:-10]).decode(), "eof": True}
        if off == 0:
            return 200, {"base64": base64.b64encode(DATA[:half]).decode(), "eof": False}
        if mode == "hang2":
            open(os.path.join(W, "hanging"), "w").close()
            time.sleep(60)
        return 500, {"error": {"code": "read_failed", "hint": "the stub refused the second range"}}
    if path == "/cli/clone-proposal/list":
        return 200, {"proposals": [{"id": "prop-1abc", "status": "open", "summary": "clone ws1", "proposedBy": "agent-a",
                                    "why": "a spare copy", "workspacePath": "/w/ws1", "target": s.get("target", "here"),
                                    "options": {}}]}
    return 200, {"ok": True}

class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        path, _, qs = self.path.partition("?")
        q = dict(p.split("=", 1) for p in qs.split("&") if "=" in p)
        with lock, open(LOG, "a") as f:
            f.write(json.dumps({"method": self.command, "path": path, "query": q,
                                "headers": dict(self.headers.items()),
                                "body": raw.decode("utf-8", "replace")}) + "\n")
        try:
            body = json.loads(raw) if raw.strip() else {}
        except ValueError:
            body = {}
        s = scn()
        status, out = answer(path, q, body, s)
        data = json.dumps(out).encode()
        self.send_response(status)
        loc = s.get("routes", {}).get(path, {}).get("location")
        if loc:
            self.send_header("Location", loc)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
    def address_string(self):
        return "stub"

if kind == "tcp":
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    with open(addr + ".tmp", "w") as f:
        f.write(str(srv.server_address[1]))
    os.rename(addr + ".tmp", addr)
else:
    class U(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
        daemon_threads = True
    srv = U(addr, H)
    open(os.path.join(W, "vault-ready"), "w").close()
srv.serve_forever()
PYEOF

"$PY" "$W/stub.py" local tcp "$W/port-local" & PIDS+=("$!")
"$PY" "$W/stub.py" remote tcp "$W/port-remote" & PIDS+=("$!")
"$PY" "$W/stub.py" other tcp "$W/port-other" & PIDS+=("$!")
"$PY" "$W/stub.py" vault unix "$KH/vault.sock" & PIDS+=("$!")
for _ in $(seq 1 100); do
    [ -s "$W/port-local" ] && [ -s "$W/port-remote" ] && [ -s "$W/port-other" ] && [ -f "$W/vault-ready" ] && break
    sleep 0.1
done
[ -s "$W/port-local" ] && [ -s "$W/port-remote" ] && [ -s "$W/port-other" ] && [ -f "$W/vault-ready" ] || { echo "FAIL: a stub never came up" >&2; exit 1; }
PL="$(cat "$W/port-local")"; PR="$(cat "$W/port-remote")"; PO="$(cat "$W/port-other")"
REMOTE="http://127.0.0.1:$PR"

# The CLI finds this server's daemon through these files.
printf '%s\n' "$PL" >"$KH/heartbeat.port"
LOCAL_TOKEN="k2lt_CLONESEC_local_token"
printf '%s\n' "$LOCAL_TOKEN" >"$KH/heartbeat.token"
printf '%s\n' "$LOCAL_TOKEN" >"$KH/daemon.token"
REMOTE_TOKEN="k2rt_CLONESEC_remote_token"

# ── The audit hook: every python3 the CLI starts logs its name lookups
# and connects (not the stubs: they started before this).
mkdir -p "$W/audit"
cat >"$W/audit/sitecustomize.py" <<'PYEOF'
import os, sys
_log = os.environ["K2T_AUDIT_LOG"]
_events = ("socket.getaddrinfo", "socket.gethostbyname", "socket.gethostbyname_ex", "socket.connect")
def _hook(ev, args):
    if ev in _events:
        with open(_log, "a") as f:
            f.write("%s %r\n" % (ev, args[1:2] if ev == "socket.connect" else args[:1]))
sys.addaudithook(_hook)
PYEOF
export PYTHONPATH="$W/audit"
export K2T_AUDIT_LOG="$W/audit.log"

# ── The pty driver: ptyrun.py MODE STEPS -- cmd…
# MODE ctty: a new session whose controlling terminal is the pty (a person
# at a terminal). MODE noctty: stdin and stderr are the pty but the session
# has no controlling terminal (/dev/tty can't be opened). Its leader is a
# small wrapper that never opens a terminal and waits for the command, so
# the command (not a session leader) can't pick one up either. STEPS is a JSON list of
# [text to wait for, bytes to type]. stdin and stderr are the terminal (as
# for a person; the CLI asks for a person when stdin or stderr is one);
# stdout goes to $W/out; what the terminal showed to $W/tty; whether echo
# is on after the run to $W/echo. On a timeout the whole session is
# killed. Exits with the command's code (128+N for signal N).
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

# ── Helpers ──────────────────────────────────────────────────────────
# fresh: empty logs, default scenarios, no clone-tmp, no saved tokens.
fresh() {
    rm -f "$W"/log-*.jsonl "$W"/scn-*.json "$W/audit.log" "$W/hanging" "$W/out" "$W/err" "$W/tty" "$W/echo"
    : >"$W/audit.log"
    rm -rf "$KH/clone-tmp" "$KH/connect-tokens.json" "$W/elsewhere"
    printf 'bundle-bytes' >"$W/bundle.tar"
    [ -S "$KH/vault.sock" ] || mv "$KH/vault.sock.off" "$KH/vault.sock"
}
scn() { printf '%s' "$2" >"$W/scn-$1.json"; }
# paths NAME: "METHOD path" per request the stub NAME got.
paths() { [ -f "$W/log-$1.jsonl" ] || return 0; "$PY" -c '
import json, sys
for l in open(sys.argv[1]):
    r = json.loads(l); print(r["method"], r["path"])' "$W/log-$1.jsonl"; }
# headers NAME: every header name the stub NAME got, one per line.
headers() { [ -f "$W/log-$1.jsonl" ] || return 0; "$PY" -c '
import json, sys
for l in open(sys.argv[1]):
    for k in json.loads(l)["headers"]: print(k)' "$W/log-$1.jsonl"; }
# k2 ARGS…: run the CLI, stdin from $IN (default /dev/null); sets RC, OUT, ERR.
IN=/dev/null
k2() {
    set +e
    "$K2" "$@" <"$IN" >"$W/out" 2>"$W/err"; RC=$?
    set -e
    OUT="$(cat "$W/out")"; ERR="$(cat "$W/err")"
}
# k2pty MODE STEPS ARGS…: the same through the pty driver; sets RC, OUT,
# TTY (what the terminal showed, stderr included; ERR is the same) and ECHO.
k2pty() {
    local mode="$1" steps="$2"; shift 2
    set +e
    "$PY" "$W/ptyrun.py" "$mode" "$steps" "$W" -- "$K2" "$@"; RC=$?
    set -e
    OUT="$(cat "$W/out")"
    TTY="$(tr -d '\r' <"$W/tty")"; ERR="$TTY"; ECHO="$(cat "$W/echo")"
}
# auth NAME PATH: the Authorization headers the stub NAME got on PATH.
auth() { [ -f "$W/log-$1.jsonl" ] || return 0; "$PY" -c '
import json, sys
for l in open(sys.argv[1]):
    r = json.loads(l)
    if r["path"] == sys.argv[2]: print(r["headers"].get("Authorization", ""))' "$W/log-$1.jsonl" "$2"; }
# no_contact LABEL: no stub got a request and no python3 looked a name up
# or connected anywhere.
no_contact() {
    expect_eq "$1: this server's daemon got nothing" "$(paths local)" ""
    expect_eq "$1: the remote got nothing" "$(paths remote)" ""
    expect_eq "$1: the vault socket got nothing" "$(paths vault)" ""
    expect_eq "$1: no name lookup or connect" "$(cat "$W/audit.log")" ""
}
# body NAME PATH: the bodies the stub NAME got on PATH.
body() { [ -f "$W/log-$1.jsonl" ] || return 0; "$PY" -c '
import json, sys
for l in open(sys.argv[1]):
    r = json.loads(l)
    if r["path"] == sys.argv[2]: print(r["body"])' "$W/log-$1.jsonl" "$2"; }
mode() { "$PY" -c 'import os, sys; print(oct(os.stat(sys.argv[1]).st_mode & 0o777)[2:])' "$1"; }
parts() { find "$KH/clone-tmp" -name '*.part' 2>/dev/null | sort; }
# bg K2-ARGS…: run the CLI in the background (BG = its pid) with SIGINT at
# its default (bash would start it with SIGINT ignored).
bg() {
    "$PY" -c 'import os, signal, sys; signal.signal(signal.SIGINT, signal.SIG_DFL); os.execv(sys.argv[1], sys.argv[1:])' \
        "$K2" "$@" </dev/null >"$W/out" 2>"$W/err" &
    BG=$!
}
bg_wait() { set +e; wait "$BG"; RC=$?; set -e; OUT="$(cat "$W/out")"; ERR="$(cat "$W/err")"; }
# hanging: wait until the remote holds the second range, then ENGINE = the
# clone engine's pid (the python3 running the stdin boot, a child of BG).
hanging() {
    local i
    for i in $(seq 1 300); do [ -f "$W/hanging" ] && break; sleep 0.1; done
    [ -f "$W/hanging" ] || { bad "the download never reached the second range"; return 1; }
    ENGINE="$(pgrep -P "$BG" -f K2_STDIN_SECRETS || true)"
    [ "$(printf '%s\n' "$ENGINE" | wc -l | tr -d ' ')" = 1 ] && [ -n "$ENGINE" ] || { bad "not exactly one clone engine under $BG: '$ENGINE'"; ps -o pid,ppid,args -e >&2; return 1; }
}
refused_line() { printf "refused: %s isn't https, so K2 won't send your token or password there; use https://%s" "$1" "$1"; }
printf '%s\n' "$REMOTE_TOKEN" >"$W/token-in"
save_token() {
    "$PY" -c 'import json,sys; json.dump({sys.argv[1]: sys.argv[2]}, open(sys.argv[3], "w"))' "$1" "$2" "$KH/connect-tokens.json"
    chmod 600 "$KH/connect-tokens.json"
}

echo "== the terminal stand-ins =="
TTYPROBE='import os
try:
    os.close(os.open("/dev/tty", os.O_RDWR | os.O_NOCTTY)); print("tty")
except OSError:
    print("no tty")'
set +e
"$PY" "$W/ptyrun.py" noctty '[]' "$W" -- "$PY" -c "$TTYPROBE"; rc=$?
set -e
expect_eq "no-terminal stand-in: /dev/tty can't be opened" "$rc $(cat "$W/out")" "0 no tty"
set +e
"$PY" "$W/ptyrun.py" ctty '[]' "$W" -- "$PY" -c "$TTYPROBE"; rc=$?
set -e
expect_eq "terminal stand-in: /dev/tty opens" "$rc $(cat "$W/out")" "0 tty"
"$PY" "$W/ptyrun.py" ctty '[]' "$W" -- "$PY" -c 'import os, termios
fd = os.open("/dev/tty", os.O_RDWR | os.O_NOCTTY); a = termios.tcgetattr(fd); a[3] &= ~termios.ECHO
termios.tcsetattr(fd, termios.TCSAFLUSH, a)'
expect_eq "terminal stand-in: echo left off is seen as off" "$(cat "$W/echo")" "off"

echo "== hand-over headers go to this computer's daemon only =="
fresh
K5_CLI_VIA=rust k2 clone to "$REMOTE" ws1 --token "$REMOTE_TOKEN" --yes --json
expect_eq "push with the front's headers succeeds" "$RC" "0"
expect_has "this server's daemon got X-K5-Cli-Via" "$(headers local)" "X-K5-Cli-Via"
expect_has "the remote got the upload" "$(paths remote)" "POST /cli/fs/upload-chunk"
if headers remote | grep -iq '^x-k5-'; then bad "the remote got an X-K5-* header: $(headers remote | grep -i '^x-k5-' | sort -u | tr '\n' ' ')"; else ok "the remote got no X-K5-* header"; fi
fresh
K5_CLI_VIA=rust k2 clone from "$REMOTE:ws1" --token "$REMOTE_TOKEN" --yes --json
expect_eq "pull with the front's headers succeeds" "$RC" "0"
expect_has "the remote got the read-range" "$(paths remote)" "GET /cli/fs/read-range"
if headers remote | grep -iq '^x-k5-'; then bad "pull: the remote got an X-K5-* header"; else ok "pull: the remote got no X-K5-* header"; fi

echo "== a token goes over https, or plain http to a loopback address as written =="
for h in box.example box.localhost 127.0.0.1.example localhost. "[::ffff:127.0.0.1]" 127.1; do
    fresh
    IN="$W/token-in" k2 clone to "http://$h:$PR" ws1 --token-stdin --yes --json
    expect_eq "to http://$h: exit 3" "$RC" "3"
    expect_eq "to http://$h: the refusal line" "$ERR" "$(refused_line "$h:$PR")"
    expect_eq "to http://$h: nothing on stdout" "$OUT" ""
    no_contact "to http://$h"
done
fresh
IN="$W/token-in" k2 clone from "http://box.localhost:$PR:ws1" --token-stdin --yes --json
expect_eq "from http://box.localhost: exit 3" "$RC" "3"
expect_eq "from http://box.localhost: the refusal line" "$ERR" "$(refused_line "box.localhost:$PR")"
no_contact "from http://box.localhost"
fresh
IN="$W/token-in" k2 clone preview ws1 --host "http://box.localhost:$PR" --token-stdin --json
expect_eq "preview --host http://box.localhost: exit 3" "$RC" "3"
expect_eq "preview --host http://box.localhost: the refusal line" "$ERR" "$(refused_line "box.localhost:$PR")"
no_contact "preview --host http://box.localhost"
fresh
IN="$W/token-in" k2 clone preview ws1 --host "http://127.0.0.1:$PR" --token-stdin --json
expect_eq "preview --host http://127.0.0.1: ok" "$RC" "0"
expect_eq "http://127.0.0.1: the remote got the token" "$(auth remote /cli/clone/preview)" "Bearer $REMOTE_TOKEN"
fresh
IN="$W/token-in" k2 clone preview ws1 --host "http://localhost:$PR" --token-stdin --json
expect_eq "preview --host http://localhost: ok" "$RC" "0"
expect_eq "http://localhost: the remote got the token" "$(auth remote /cli/clone/preview)" "Bearer $REMOTE_TOKEN"
for h in 127.0.0.2 "[::1]"; do
    fresh
    IN="$W/token-in" k2 clone preview ws1 --host "http://$h:1" --token-stdin --json
    expect_eq "http://$h (nothing listening): exit 3" "$RC" "3"
    expect_has "http://$h: allowed, so it tried and found nothing" "$ERR" "daemon_unreachable"
    expect_not "http://$h: not refused" "$ERR" "refused:"
done

echo "== a password never goes over plain http, even to loopback =="
fresh
save_token "$REMOTE" "$REMOTE_TOKEN"
k2pty ctty '[]' clone from "$REMOTE:ws1" --copy API_KEY --json
expect_eq "pull with a password confirm over http: exit 3" "$RC" "3"
expect_has "pull with a password confirm over http: the refusal line" "$ERR" "$(refused_line "127.0.0.1:$PR")"
expect_not "no password prompt on the terminal" "$TTY" "Your password on"
expect_not "the remote got no password confirm" "$(paths remote)" "/cli/clone/vault/confirm"
expect_not "the remote packed nothing" "$(paths remote)" "/cli/clone/pack"

echo "== this computer's confirm goes through the vault socket only =="
fresh
mv "$KH/vault.sock" "$KH/vault.sock.off"
IN="$W/token-in" k2 clone to "$REMOTE" ws1 --copy API_KEY --token-stdin --json
expect_eq "push with a copy and no vault socket: exit 3" "$RC" "3"
expect_has "push with no vault socket: vault_unreachable" "$ERR" '"error": "vault_unreachable"'
expect_not "push: no confirm over TCP" "$(paths local)" "confirm"
expect_not "push: nothing bundled" "$(paths local)" "/cli/clone/bundle"
expect_not "push: nothing uploaded" "$(paths remote)" "/cli/fs/upload-chunk"
fresh
mv "$KH/vault.sock" "$KH/vault.sock.off"
scn local '{"target": "here"}'
k2 clone approve prop-1abc --json
expect_eq "approve with no vault socket: exit 3" "$RC" "3"
expect_has "approve with no vault socket: vault_unreachable" "$ERR" '"error": "vault_unreachable"'
expect_not "approve: no confirm over TCP" "$(paths local)" "confirm"
expect_not "approve: nothing approved" "$(paths local)" "/cli/clone-proposal/approve"

echo "== a password comes from a real terminal only, echo off =="
fresh
save_token "$REMOTE" "$REMOTE_TOKEN"
k2pty noctty '[]' clone to "$REMOTE" ws1 --copy API_KEY --json
expect_eq "no controlling terminal: exit 3" "$RC" "3"
expect_has "no controlling terminal: confirm_needs_a_person" "$ERR" '"error": "confirm_needs_a_person"'
expect_not "no controlling terminal: no fallback warning" "$ERR" "Warning"
expect_not "no controlling terminal: no traceback" "$ERR" "Traceback"
expect_not "no controlling terminal: no confirm finished" "$(paths vault)" "confirm/finish"
expect_not "no controlling terminal: nothing bundled" "$(paths local)" "/cli/clone/bundle"
fresh
save_token "$REMOTE" "$REMOTE_TOKEN"
k2pty ctty '[["Vault password", "pw-CLONESEC-typed\n"]]' clone to "$REMOTE" ws1 --copy API_KEY --json
expect_eq "at a terminal: the push finishes" "$RC" "0"
expect_has "at a terminal: the typed password reached the vault socket" "$(body vault /cli/vault/confirm/finish)" "pw-CLONESEC-typed"
expect_not "at a terminal: the password was not echoed" "$TTY" "pw-CLONESEC-typed"
expect_eq "at a terminal: echo is on again after" "$ECHO" "on"

echo "== Ctrl-C at the password prompt =="
fresh
save_token "$REMOTE" "$REMOTE_TOKEN"
k2pty ctty '[["Vault password", "\u0003"]]' clone to "$REMOTE" ws1 --copy API_KEY --json
expect_eq "Ctrl-C: exit 130" "$RC" "130"
expect_has "Ctrl-C: the cancelled line" "$ERR" '{"error": "cancelled", "hint": "stopped"}'
expect_eq "Ctrl-C: echo is on again after" "$ECHO" "on"
expect_not "Ctrl-C: no confirm finished" "$(paths vault)" "confirm/finish"
expect_not "Ctrl-C: nothing bundled" "$(paths local)" "/cli/clone/bundle"

echo "== clone-tmp and the download's .part =="
fresh
k2 clone from "$REMOTE:ws1" --token "$REMOTE_TOKEN" --yes --json
expect_eq "pull: ok" "$RC" "0"
expect_eq "a new clone-tmp is 0700" "$(mode "$KH/clone-tmp")" "700"
expect_eq "the downloaded bundle is 0600" "$(mode "$KH/clone-tmp/bundle-job-1.tar")" "600"
expect_has "this server unpacked the download" "$(body local /cli/clone/unpack)" "$KH/clone-tmp/bundle-job-1.tar"
fresh
mkdir -m 755 "$KH/clone-tmp"
chmod 755 "$KH/clone-tmp"
k2 clone from "$REMOTE:ws1" --token "$REMOTE_TOKEN" --yes --json
expect_eq "pull into an old 0755 clone-tmp: ok" "$RC" "0"
expect_eq "an old 0755 clone-tmp is 0700 after" "$(mode "$KH/clone-tmp")" "700"
fresh
mkdir "$W/elsewhere"
ln -s "$W/elsewhere" "$KH/clone-tmp"
k2 clone from "$REMOTE:ws1" --token "$REMOTE_TOKEN" --yes --json
expect_eq "clone-tmp is a link: exit 3" "$RC" "3"
expect_has "clone-tmp is a link: clone_tmp_unsafe" "$ERR" '"error": "clone_tmp_unsafe"'
expect_eq "clone-tmp is a link: nothing written where it points" "$(ls -A "$W/elsewhere")" ""
expect_has "clone-tmp is a link: the remote pack is cleaned up" "$(paths remote)" "POST /cli/clone/pack-cleanup"
fresh
scn remote '{"rr": "fail2"}'
k2 clone from "$REMOTE:ws1" --token "$REMOTE_TOKEN" --yes --json
expect_eq "a refused second range: exit 3" "$RC" "3"
expect_has "a refused second range: read_failed" "$ERR" "read_failed"
expect_eq "a refused second range: no .part left" "$(parts)" ""
expect_has "a refused second range: the remote pack is cleaned up" "$(paths remote)" "POST /cli/clone/pack-cleanup"
fresh
scn remote '{"rr": "short"}'
k2 clone from "$REMOTE:ws1" --token "$REMOTE_TOKEN" --yes --json
expect_eq "a short download: exit 3" "$RC" "3"
expect_has "a short download: download_failed" "$ERR" '"error": "download_failed"'
expect_eq "a short download: no .part left" "$(parts)" ""
for sig in TERM INT; do
    fresh
    scn remote '{"rr": "hang2"}'
    bg clone from "$REMOTE:ws1" --token "$REMOTE_TOKEN" --yes --json
    if hanging; then
        expect_eq "SIG$sig: the .part is 0600 while it is written" "$(mode "$KH/clone-tmp/bundle-job-1.tar.part")" "600"
        kill -"$sig" "$ENGINE"
    fi
    bg_wait
    if [ "$sig" = TERM ]; then
        expect_eq "SIGTERM: exit 143" "$RC" "143"
    else
        expect_eq "Ctrl-C (SIGINT): exit 130" "$RC" "130"
        expect_has "Ctrl-C (SIGINT): the cancelled line" "$ERR" '{"error": "cancelled", "hint": "stopped"}'
    fi
    expect_eq "SIG$sig: no .part left" "$(parts)" ""
    expect_has "SIG$sig: the remote pack is cleaned up" "$(paths remote)" "POST /cli/clone/pack-cleanup"
done

echo "== the remote token on stdin; --token with its note =="
fresh
k2 clone preview ws1 --host "$REMOTE" --token "$REMOTE_TOKEN" --json
expect_eq "--token still works" "$RC" "0"
expect_has "--token prints its note" "$ERR" "note: --token is visible to every account on this computer while the command runs; use --token-stdin"
fresh
IN="$W/token-in" k2 clone preview ws1 --host "$REMOTE" --token-stdin --json
expect_eq "--token-stdin works" "$RC" "0"
expect_eq "--token-stdin: the remote got the token" "$(auth remote /cli/clone/preview)" "Bearer $REMOTE_TOKEN"
expect_not "--token-stdin: no note" "$ERR" "note:"
fresh
IN="$W/token-in" k2 clone preview ws1 --host "$REMOTE" --token-stdin --token "$REMOTE_TOKEN" --json
expect_eq "--token-stdin and --token: usage, exit 2" "$RC" "2"
expect_eq "--token-stdin and --token: nothing sent" "$(paths remote)" ""

echo "== clone approve: the person says yes before a token goes to the proposed host =="
fresh
scn local "{\"target\": \"$REMOTE\"}"
K2_REMOTE_TOKEN="k2rt_CLONESEC_env_token" k2pty ctty '[]' clone approve prop-1abc --json
expect_eq "no saved token for the host: exit 3" "$RC" "3"
expect_has "no saved token for the host: says how to sign in" "$ERR" '"error": "no_saved_token"'
expect_has "no saved token for the host: names the host" "$ERR" "$REMOTE"
expect_eq "no saved token: the remote got nothing (not the env token)" "$(paths remote)" ""
fresh
scn local "{\"target\": \"$REMOTE\"}"
save_token "$REMOTE" "$REMOTE_TOKEN"
k2pty ctty '[["Send your token to ", "n\n"]]' clone approve prop-1abc --json
expect_eq "the person says no: exit 3" "$RC" "3"
expect_has "the person says no: not_approved" "$ERR" '"error": "not_approved"'
expect_has "the confirm names the host first" "$TTY" "<<Send your token to >>$REMOTE?"
expect_eq "the person says no: the remote got nothing" "$(paths remote)" ""
expect_not "the person says no: nothing approved" "$(paths local)" "/cli/clone-proposal/approve"
fresh
scn local "{\"target\": \"$REMOTE\"}"
save_token "$REMOTE" "$REMOTE_TOKEN"
k2pty noctty '[]' clone approve prop-1abc --yes --json
expect_eq "no terminal (even with --yes): exit 3" "$RC" "3"
expect_has "no terminal: confirm_needs_a_person" "$ERR" '"error": "confirm_needs_a_person"'
expect_eq "no terminal: the remote got nothing" "$(paths remote)" ""
fresh
# No secret rows, so the only questions are the host and the approve.
scn local "{\"target\": \"$REMOTE\", \"rows\": []}"
save_token "$REMOTE" "$REMOTE_TOKEN"
k2pty ctty '[["Send your token to ", "y\n"], ["Vault password", "pw-CLONESEC-typed\n"]]' clone approve prop-1abc --json
expect_eq "the person says yes: the clone runs" "$RC" "0"
expect_has "the person says yes: the remote got the upload" "$(paths remote)" "POST /cli/fs/upload-chunk"
expect_eq "the person says yes: the remote got the saved token" "$(auth remote /cli/fs/info)" "Bearer $REMOTE_TOKEN"
expect_has "the person says yes: approved" "$(paths local)" "POST /cli/clone-proposal/approve"
LONG="acme-build-box-in-the-north-rack.globex.example.test"
fresh
scn local "{\"target\": \"$LONG\"}"
save_token "$LONG" "$REMOTE_TOKEN"
k2pty ctty '[["Send your token to ", "n\n"]]' clone approve prop-1abc --json
expect_has "the confirm shows the proposed host in full" "$TTY" "<<Send your token to >>$LONG? (https://$LONG)"
expect_not "the long host: no lookup of it" "$(cat "$W/audit.log")" "$LONG"
expect_eq "the long host: the remote got nothing" "$(paths remote)" ""
fresh
scn local '{"target": "http://box.example"}'
save_token "http://box.example" "$REMOTE_TOKEN"
k2pty ctty '[]' clone approve prop-1abc --json
expect_eq "an http proposed host: exit 3" "$RC" "3"
expect_has "an http proposed host: the refusal line" "$ERR" "$(refused_line box.example)"
expect_not "an http proposed host: no lookup" "$(cat "$W/audit.log")" "box.example"
for f in "--token $REMOTE_TOKEN" "--token-stdin"; do
    fresh
    scn local "{\"target\": \"$REMOTE\"}"
    # shellcheck disable=SC2086
    IN="$W/token-in" k2 clone approve prop-1abc $f --json
    expect_eq "approve $f: usage, exit 2" "$RC" "2"
    expect_eq "approve $f: the remote got nothing" "$(paths remote)" ""
done

echo "== no request follows a redirect =="
for to in "http://127.0.0.1:$PO/elsewhere" "https://127.0.0.1:$PO/other-host"; do
    fresh
    scn remote "{\"routes\": {\"/cli/clone/preview\": {\"status\": 302, \"location\": \"$to\"}}}"
    IN="$W/token-in" k2 clone preview ws1 --host "$REMOTE" --token-stdin --json
    expect_eq "302 to $to: exit 3" "$RC" "3"
    expect_has "302 to $to: redirect_refused" "$ERR" '"error": "redirect_refused"'
    expect_has "302 to $to: names where it pointed" "$ERR" "sending K2 to 127.0.0.1:$PO"
    expect_eq "302 to $to: the remote got the one request" "$(paths remote)" "POST /cli/clone/preview"
    expect_eq "302 to $to: the place it named got nothing" "$(paths other)" ""
    expect_not "302 to $to: no connect to it" "$(cat "$W/audit.log")" ", $PO)"
done

echo "== an address with a name before an @ is refused, and the name never shown =="
for spec in "to|https://real-k2-dev.example@evil.example" "to|http://user:secret-CLONESEC@127.0.0.1:$PR" "preview|https://real-k2-dev.example@127.0.0.1:$PR"; do
    verb="${spec%%|*}"; url="${spec#*|}"
    fresh
    if [ "$verb" = to ]; then
        IN="$W/token-in" k2 clone to "$url" ws1 --token-stdin --yes --json
    else
        IN="$W/token-in" k2 clone preview ws1 --host "$url" --token-stdin --json
    fi
    expect_eq "$verb $url: exit 3" "$RC" "3"
    expect_has "$verb $url: refused for the name before the @" "$ERR" "has a name before an @"
    expect_not "$verb $url: the output shows no user info (real-k2-dev)" "$OUT$ERR" "real-k2-dev"
    expect_not "$verb $url: the output shows no user info (secret)" "$OUT$ERR" "secret-CLONESEC"
    expect_not "$verb $url: the output shows no user info (user:)" "$OUT$ERR" "user:"
    no_contact "$verb $url"
done
fresh
scn local '{"target": "https://real-k2-dev.example@evil.example"}'
save_token "https://real-k2-dev.example@evil.example" "$REMOTE_TOKEN"
k2pty ctty '[]' clone approve prop-1abc --json
expect_eq "a proposal target with a name before an @: exit 3" "$RC" "3"
expect_has "a proposal target with a name before an @: refused" "$ERR" "has a name before an @"
expect_not "a proposal target with a name before an @: no user info shown" "$OUT$ERR" "real-k2-dev"
expect_eq "a proposal target with a name before an @: the remote got nothing" "$(paths remote)" ""
expect_not "a proposal target with a name before an @: no lookup" "$(cat "$W/audit.log")" "evil.example"
fresh
IN="$W/token-in" k2 clone preview ws1 --host "http://127.0.0.1:$PR/a@b" --token-stdin --json
expect_eq "an @ in the path, after the host: allowed" "$RC" "0"
expect_has "an @ in the path, after the host: the remote got the request" "$(paths remote)" "POST /a@b/cli/clone/preview"

echo "== a Tailscale address as written may take the token over plain http =="
# The engine's own rule, read from the CLI under test.
eval "$(awk "/^_CLONE_URL_RULE='/,/^'\$/" "$K2")"
[ -n "${_CLONE_URL_RULE:-}" ] || { bad "no _CLONE_URL_RULE in $K2"; }
rule() { "$PY" -c "$_CLONE_URL_RULE
import sys
print(clone_url_refusal(sys.argv[1], sys.argv[2] == \"password\") or \"ok\")" "$1" "$2"; }
for h in 100.64.0.1 "[fd7a:115c:a1e0::1]"; do
    fresh
    save_token "http://$h:9" "$REMOTE_TOKEN"
    bg clone preview ws1 --host "http://$h:9" --json
    for _ in $(seq 1 150); do grep -Fq "${h//[\[\]]/}" "$W/audit.log" && break; kill -0 "$BG" 2>/dev/null || break; sleep 0.1; done
    pkill -P "$BG" 2>/dev/null || true
    kill "$BG" 2>/dev/null || true
    bg_wait
    expect_has "http://$h: allowed, so the engine connects there" "$(cat "$W/audit.log")" "${h//[\[\]]/}"
    expect_not "http://$h: not refused" "$ERR" "refused:"
    expect_not "http://$h: no warning" "$ERR" "warning:"
    expect_eq "http://$h: a password still refused" "$(rule "http://$h:9" password)" "$(refused_line "$h:9")"
done
for h in box.ts.net 100.128.0.1 100.63.255.255 "[fd7a:115c:a1e1::1]"; do
    fresh
    IN="$W/token-in" k2 clone preview ws1 --host "http://$h:9" --token-stdin --json
    expect_eq "http://$h: exit 3" "$RC" "3"
    expect_eq "http://$h: the refusal line" "$ERR" "$(refused_line "$h:9")"
    no_contact "http://$h"
done

echo "== this server's side over plain http warns once =="
ME="$(basename "$K2")"
warn_line() { printf "warning: %s is sending this server's owner token to %s over plain http, unencrypted. That's fine on a private network such as a tailnet; elsewhere use https." "$ME" "$1"; }
for h in 192.0.2.10 box.lan box.ts.net.evil.example 100.128.0.1; do
    fresh
    K2_HOST="$h" k2 clone to http://box.example ws1 --json
    expect_eq "K2_HOST=$h: refused as before (exit 3)" "$RC" "3"
    expect_eq "K2_HOST=$h: the warning first" "$(printf '%s\n' "$ERR" | head -1)" "$(warn_line "$h")"
    expect_eq "K2_HOST=$h: one warning" "$(printf '%s\n' "$ERR" | grep -c '^warning:')" "1"
done
for h in 127.0.0.1 localhost ::1 100.100.1.2 fd7a:115c:a1e0::5 box.tail1234.ts.net; do
    fresh
    K2_HOST="$h" k2 clone to http://box.example ws1 --json
    expect_not "K2_HOST=$h: no warning" "$ERR" "warning:"
done
# https to this server is not plain http, whatever the host: no warning.
for h in https://192.0.2.10 https://box.lan HTTPS://box.lan; do
    fresh
    K2_HOST="$h" k2 clone to http://box.example ws1 --json
    expect_eq "K2_HOST=$h: refused as before (exit 3)" "$RC" "3"
    expect_not "K2_HOST=$h: no warning" "$ERR" "warning:"
done
fresh
K2_HOST=192.0.2.10 K2_PLAIN_HTTP_OK=1 k2 clone to http://box.example ws1 --json
expect_not "K2_PLAIN_HTTP_OK=1: no warning" "$ERR" "warning:"
fresh
K2_HOST=192.0.2.10 k2 clone to -h
expect_eq "help: exit 0" "$RC" "0"
expect_not "help: no warning" "$OUT$ERR" "warning:"

echo "== the help says what the confirms are, and aren't =="
fresh
k2 clone --help
expect_eq "clone --help: exit 0" "$RC" "0"
expect_has "the help: the confirms ask whoever is at the terminal" "$OUT" "The confirms ask whoever is at that terminal."
expect_has "the help: not a security boundary" "$OUT" "They are not a security"
expect_has "the help: against programs run as the same user" "$OUT" "boundary against programs run as the same user"
expect_not "the help: no claim that agents can't clone" "$OUT" "Agents can't clone"

echo
echo "clone_security: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
