#!/usr/bin/env bash
# A password never goes over plain http to another computer, from a flag
# or from a prompt (security fix). With K2_HOST a name or address that
# isn't loopback as written (x.localhost here: curl sends it to loopback,
# where the stub is), every prompt below for a password, PIN, API key or
# secret value is refused BEFORE it asks:
#   Error: a password never goes over plain http:// to another computer; use K2_HOST=https://<name> (got: http://x.localhost:<port>)
# as the last stderr line, exit 2, the prompt never shown, the typed line
# never read, and nothing sent after the point where the prompt would be.
# K2_PLAIN_HTTP_OK=1 and a tailnet host don't lift it (the password-flag
# rule is the same; tests/cli/connect_base_and_proxy.sh holds its rows).
# Loopback (K2_HOST unset) still prompts and the typed value reaches the
# stub; so does an https K2_HOST where the prompt comes before any TCP
# request (an https K2_HOST can't reach this plain stub, so the PIN and
# pair-phone prompts, which follow a request, have loopback rows only).
#
# The vault confirm prompts (vault rollback here) run inside $(...) and
# every caller of a failed confirm exits 3, so those rows want exit 3:
# the line and the nothing-read, nothing-sent checks are the same.
#
# stdin is a pty (so the CLI asks a person) with the typed line already
# waiting in it, echo off; stdout and stderr are files. After the run the
# driver reads what is left in the pty: the whole typed line means the
# CLI read nothing.
#
# Sites (cli/src):
#   job.sh            vault plan confirm: vault or K2 Connect password
#   vault.sh          vault confirm: vault password, owner login password;
#                     a secret value typed in (vault set)
#   migrate.sh        vault confirm-password set (new vault password),
#                     vault restore (recovery words)
#   app-auth.sh       notifications email relay add (relay password)
#   connect.sh        publish visibility ... pin (PIN)
#   llm.sh            llm tokens add-key (API token)
#   users-phones.sh   users pair-phone (login password)
#   clone.sh          users add, users set-password (a login's password)
#   talk-and-workspace.sh  workspace api-key --key-stdin, api-key create
#                     --key-stdin (LLM API keys)
#
# Hermetic: throwaway HOME, every K2_*/K5_*/K2SO_* var unset, stub daemon
# (loopback TCP) and stub vault socket. Opens ports: run it on K2 Compute.
#
#   bash tests/cli/password_prompt_plain_http.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
PY="$(command -v python3)" || { echo "FAIL: python3 required" >&2; exit 1; }
command -v curl >/dev/null || { echo "FAIL: curl required" >&2; exit 1; }

for v in $(env | grep -E '^(K2|K5|K2SO)_' | cut -d= -f1); do unset "$v"; done
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY PYTHONPATH

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

# Short: the vault socket path must fit a sun_path.
W="$(mktemp -d /tmp/k2pw-XXXXXX)"
PIDS=()
cleanup() {
    for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
    rm -rf "$W"
}
trap cleanup EXIT

export HOME="$W/home"
KH="$HOME/.k2"
mkdir -p "$KH" "$W/proj" "$W/backup" "$W/bin"
: >"$W/backup/vault.db"

# ── The stubs: stub.py NAME tcp PORTFILE | NAME unix SOCKPATH. Each logs
# every request (method, path, body) to $W/log-NAME.jsonl. $W/scn.json
# (read on every request) sets the vault knobs: placements (vault show),
# methods and login (confirm/start).
cat >"$W/stub.py" <<'PYEOF'
import http.server, json, os, socketserver, sys, threading
name, kind, addr = sys.argv[1], sys.argv[2], sys.argv[3]
W = os.path.dirname(os.path.abspath(__file__))
LOG = os.path.join(W, "log-%s.jsonl" % name)
lock = threading.Lock()

def scn():
    try:
        with open(os.path.join(W, "scn.json")) as f:
            return json.load(f)
    except FileNotFoundError:
        return {}

def answer(path, body, s):
    if path == "/cli/vault/show":
        return {"secret": {"id": "sec-1", "name": "API_KEY"}, "placements": s.get("placements", [])}
    if path == "/cli/vault/confirm/start":
        return {"methods": s.get("methods", ["vault-password"]), "login": s.get("login", ""), "expiresIn": 60}
    if path == "/cli/vault/confirm/finish":
        return {"token": "VT-1"}
    if path == "/cli/vault/status":
        return {"confirmPassword": False}
    if path == "/cli/vault/restore":
        return {"vaultId": "vault-1", "secrets": 1, "previous": "/tmp/old.db"}
    if path == "/cli/vault/set":
        return {"created": True, "secret": {"name": "API_KEY", "latestVersion": 1}, "ref": "k2v:API_KEY", "use": "-"}
    if path == "/cli/auth/device/pair-code":
        if "password" in body:
            return {"code": "123456", "qr": {}, "expiresInSecs": 60}
        return {"error": {"code": "password_required", "hint": "type your password again to make a pairing code"}}
    if path == "/cli/publish/visibility":
        if "pin" in body:
            return {"ok": True, "site": {"label": "acme", "visibility": "pin"}}
        return {"ok": True, "check": True}
    if path == "/cli/notifications/email/relay/add":
        return {"relay": {"id": "r1", "host": "smtp.example.test", "port": 587, "tls": "starttls", "fromDomain": "example.test"}, "next": "-"}
    if path == "/cli/workspace/resolve":
        return {"path": "/w/ws1"}
    if path == "/cli/workspace/api-key":
        return {"keySet": True}
    return {"ok": True, "success": True}

class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        path = self.path.split("?", 1)[0]
        with lock, open(LOG, "a") as f:
            f.write(json.dumps({"method": self.command, "path": path,
                                "body": raw.decode("utf-8", "replace")}) + "\n")
        try:
            body = json.loads(raw) if raw.strip() else {}
        except ValueError:
            body = {}
        if not isinstance(body, dict):
            body = {}
        data = json.dumps(answer(path, body, scn())).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)
    do_GET = do_POST = do_PUT = do_DELETE = _any
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

"$PY" "$W/stub.py" daemon tcp "$W/port" & PIDS+=("$!")
"$PY" "$W/stub.py" vault unix "$KH/vault.sock" & PIDS+=("$!")
for _ in $(seq 1 100); do
    [ -s "$W/port" ] && [ -f "$W/vault-ready" ] && break
    sleep 0.1
done
[ -s "$W/port" ] && [ -f "$W/vault-ready" ] || { echo "FAIL: a stub never came up" >&2; exit 1; }
DPORT="$(cat "$W/port")"
OWNER="owner-pwprompt-1111"
printf '%s\n' "$DPORT" >"$KH/heartbeat.port"
printf '%s\n' "$OWNER" >"$KH/heartbeat.token"
printf '%s\n' "$OWNER" >"$KH/daemon.token"
chmod 600 "$KH/heartbeat.token" "$KH/daemon.token"

# Unresolvable https hosts: curl gives up quickly (the last
# --connect-timeout and --max-time win).
REAL_CURL="$(command -v curl)"
printf '#!/bin/sh\nexec "%s" "$@" --connect-timeout 2 --max-time 3\n' "$REAL_CURL" >"$W/bin/curl"
chmod +x "$W/bin/curl"

# ── The pty driver: ptyrun.py W STEPS -- cmd…
# STEPS "-": stdin is a pty with echo off and $W/typed already waiting in
# it. Else a JSON list of [text, bytes]: each time the text shows on
# stderr, the bytes are typed (for Python's getpass, which flushes what
# was typed before it asks). The
# command runs in a new session with no controlling terminal (its leader is
# a small wrapper that waits for it). stdout to $W/out, stderr to $W/err;
# what the command left unread in the pty to $W/unread. Exits with the
# command's code (128+N for signal N), or 124 when it never finished in
# 30 s (killed; the rows then fail).
cat >"$W/ptyrun.py" <<'PYEOF'
import json, os, subprocess, sys, termios, time
W = sys.argv[1]
steps = [] if sys.argv[2] == "-" else json.loads(sys.argv[2])
cmd = sys.argv[sys.argv.index("--") + 1:]
wrap = [sys.executable, "-c", """
import os, sys
pid = os.fork()
if pid == 0:
    os.execvp(sys.argv[1], sys.argv[1:])
_, st = os.waitpid(pid, 0)
sys.exit(os.WEXITSTATUS(st) if os.WIFEXITED(st) else 128 + os.WTERMSIG(st))
"""] + cmd
m, s = os.openpty()
a = termios.tcgetattr(s)
a[3] &= ~termios.ECHO
termios.tcsetattr(s, termios.TCSANOW, a)
if not steps:
    with open(os.path.join(W, "typed"), "rb") as f:
        os.write(m, f.read())
with open(os.path.join(W, "out"), "wb") as out, open(os.path.join(W, "err"), "wb") as err:
    p = subprocess.Popen(wrap, stdin=s, stdout=out, stderr=err, preexec_fn=os.setsid)
    end = time.time() + 30
    timed_out = False
    seen = 0
    while p.poll() is None:
        if steps:
            with open(os.path.join(W, "err"), "rb") as f:
                shown = f.read()
            at = shown.find(steps[0][0].encode(), seen)
            if at >= 0:
                seen = at + len(steps[0][0])
                os.write(m, steps.pop(0)[1].encode())
        if time.time() > end:
            os.killpg(p.pid, 9)
            p.wait()
            timed_out = True
            break
        time.sleep(0.05)
os.set_blocking(s, False)
left = b""
while True:
    try:
        chunk = os.read(s, 65536)
    except (BlockingIOError, OSError):
        break
    if not chunk:
        break
    left += chunk
with open(os.path.join(W, "unread"), "wb") as f:
    f.write(left)
if timed_out:
    sys.stderr.write("ptyrun: the command never finished (30 s)\n")
    sys.exit(124)
sys.exit(p.returncode)
PYEOF

# ── Helpers
SECRET="pw-PWPROMPT-typed-5555"
SECRET_B64="$(printf '%s' "$SECRET" | "$PY" -c 'import base64,sys; print(base64.b64encode(sys.stdin.buffer.read()).decode())')"
want_line() { printf 'Error: a password never goes over plain http:// to another computer; use K2_HOST=https://<name> (got: %s)' "$1"; }
WANT="$(want_line "http://x.localhost:$DPORT")"

# run [ENV=V…] -- ARGS…: the CLI with the pty as stdin and $W/typed
# waiting in it. Sets RC, OUT, ERR, UNREAD (what it left unread).
TYPED_LINES=1
STEPS=-
run() {
    local -a envs=(K2_PROJECT_PATH="$W/proj")
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    rm -f "$W"/log-*.jsonl "$W/out" "$W/err" "$W/unread"
    : >"$W/typed"
    local i
    for i in $(seq 1 "$TYPED_LINES"); do printf '%s\n' "$SECRET" >>"$W/typed"; done
    RC=0
    env "${envs[@]}" "$PY" "$W/ptyrun.py" "$W" "$STEPS" -- "$K2" "$@" || RC=$?
    OUT="$(cat "$W/out")"
    ERR="$(cat "$W/err")"
    UNREAD="$(tr -d '\r' <"$W/unread")"
}
scn() { printf '%s' "$1" >"$W/scn.json"; }
# reqs: "NAME METHOD path" per request, both stubs.
reqs() {
    local n
    for n in daemon vault; do
        [ -f "$W/log-$n.jsonl" ] || continue
        "$PY" -c '
import json, sys
for l in open(sys.argv[1]):
    r = json.loads(l); print(sys.argv[2], r["method"], r["path"])' "$W/log-$n.jsonl" "$n"
    done
}
# secret_sent: true when the typed value (or its base64) reached a stub.
secret_sent() { cat "$W"/log-*.jsonl 2>/dev/null | grep -Fq -e "$SECRET" -e "$SECRET_B64"; }

# refused LABEL WANT_RC PROMPT WANT_REQS: the prompt was never shown, the
# typed line was never read, the last stderr line is the refusal, the exit
# code is WANT_RC, the stubs got exactly WANT_REQS (newline-separated, may
# be empty) and never the typed value.
refused() {
    local label="$1" want_rc="$2" prompt="$3" want_reqs="$4" got_reqs last
    [ "$RC" != 124 ] || bad "$label: never finished (stderr: $(printf %q "$ERR"))"
    [ "$RC" = "$want_rc" ] && ok "$label: exit $want_rc" || bad "$label: exit $RC, want $want_rc (stderr: $(printf %q "$ERR"))"
    last="$(printf '%s\n' "$ERR" | tail -n 1)"
    [ "$last" = "$WANT" ] && ok "$label: the refusal line" || bad "$label: last stderr line $(printf %q "$last"), want $(printf %q "$WANT")"
    case "$ERR" in
        *"$prompt"*) bad "$label: the prompt was shown: $(printf %q "$ERR")" ;;
        *) ok "$label: the prompt was never shown" ;;
    esac
    if [ "$UNREAD" = "$(tr -d '\r' <"$W/typed")" ]; then
        ok "$label: nothing was read from the terminal"
    else
        bad "$label: the CLI read from the terminal (left: $(printf %q "$UNREAD"))"
    fi
    got_reqs="$(reqs)"
    [ "$got_reqs" = "$want_reqs" ] && ok "$label: only the requests before the prompt" || bad "$label: requests $(printf %q "$got_reqs"), want $(printf %q "$want_reqs")"
    if secret_sent; then bad "$label: the typed value was sent"; else ok "$label: the typed value was not sent"; fi
}
# prompted LABEL PROMPT [ROUTE]: the prompt was shown, no refusal, the
# typed line was read, and (with ROUTE "NAME path") the typed value
# reached that stub route.
prompted() {
    local label="$1" prompt="$2" route="${3:-}"
    [ "$RC" != 124 ] && ok "$label: finished" || bad "$label: never finished (stderr: $(printf %q "$ERR"))"
    case "$ERR" in
        *"$prompt"*) ok "$label: the prompt was shown" ;;
        *) bad "$label: no prompt (exit $RC, stderr: $(printf %q "$ERR"))" ;;
    esac
    case "$ERR" in
        *"a password never goes over plain http"*) bad "$label: refused: $(printf %q "$ERR")" ;;
        *) ok "$label: not refused" ;;
    esac
    [ -z "$UNREAD" ] && ok "$label: the typed line was read" || bad "$label: left unread: $(printf %q "$UNREAD")"
    if [ -n "$route" ]; then
        local n="${route%% *}" p="${route#* }"
        if [ -f "$W/log-$n.jsonl" ] && "$PY" -c '
import json, sys
for l in open(sys.argv[1]):
    r = json.loads(l)
    if r["path"] == sys.argv[2] and (sys.argv[3] in r["body"] or sys.argv[4] in r["body"]):
        sys.exit(0)
sys.exit(1)' "$W/log-$n.jsonl" "$p" "$SECRET" "$SECRET_B64"; then
            ok "$label: the typed value reached $route"
        else
            bad "$label: the typed value never reached $route ($(reqs | tr '\n' ';'))"
        fi
    fi
}

PLAIN=(K2_HOST=x.localhost)
HTTPS=(K2_HOST=https://k2-lan.example.test PATH="$W/bin:$PATH")

# curl must send x.localhost to loopback here, or the rows mean nothing.
rm -f "$W"/log-*.jsonl
"$REAL_CURL" -s -o /dev/null --max-time 5 --noproxy '*' "http://x.localhost:$DPORT/probe" || true
if [ "$(reqs)" = "daemon GET /probe" ]; then ok "curl sends x.localhost to loopback"; else bad "curl did not send x.localhost to the stub: $(reqs | tr '\n' ';')"; fi

echo "== job.sh: the vault plan confirm (vault rollback of a secret a root unit loads) =="
P='Vault password: '
scn '{"placements": [{"target": "systemd_cred", "version": 1}], "methods": ["vault-password"]}'
run "${PLAIN[@]}" -- vault rollback API_KEY --to 1
refused "plan confirm, vault password" 3 "$P" "vault GET /cli/vault/show
vault POST /cli/vault/confirm/start"
run -- vault rollback API_KEY --to 1
prompted "plan confirm, vault password, loopback" "$P" "vault /cli/vault/confirm/finish"
run "${HTTPS[@]}" -- vault rollback API_KEY --to 1
prompted "plan confirm, vault password, https" "$P" "vault /cli/vault/confirm/finish"
P='Your K2 Connect password: '
scn '{"placements": [{"target": "systemd_cred", "version": 1}], "methods": ["connect-password"]}'
run "${PLAIN[@]}" -- vault rollback API_KEY --to 1
refused "plan confirm, Connect password" 3 "$P" "vault GET /cli/vault/show
vault POST /cli/vault/confirm/start"
run -- vault rollback API_KEY --to 1
prompted "plan confirm, Connect password, loopback" "$P" "vault /cli/vault/confirm/finish"

echo "== vault.sh: the vault confirm (vault rollback) =="
P='Vault password: '
scn '{"placements": [], "methods": ["vault-password"]}'
run "${PLAIN[@]}" -- vault rollback API_KEY --to 1
refused "confirm, vault password" 3 "$P" "vault GET /cli/vault/show
vault POST /cli/vault/confirm/start"
run -- vault rollback API_KEY --to 1
prompted "confirm, vault password, loopback" "$P" "vault /cli/vault/confirm/finish"
P='Sign-in password for owner login admin: '
scn '{"placements": [], "methods": ["owner-login-password"], "login": "admin"}'
run "${PLAIN[@]}" -- vault rollback API_KEY --to 1
refused "confirm, owner login password" 3 "$P" "vault GET /cli/vault/show
vault POST /cli/vault/confirm/start"
run -- vault rollback API_KEY --to 1
prompted "confirm, owner login password, loopback" "$P" "vault /cli/vault/confirm/finish"
# No login named: the login question is not asked either.
scn '{"placements": [], "methods": ["owner-login-password"]}'
run "${PLAIN[@]}" -- vault rollback API_KEY --to 1
refused "confirm, owner login password, no login named" 3 "Which owner login" "vault GET /cli/vault/show
vault POST /cli/vault/confirm/start"

echo "== vault.sh: a secret value typed in (vault set) =="
P='Value (not shown): '
scn '{}'
run "${PLAIN[@]}" -- vault set API_KEY
refused "vault set value" 2 "$P" ""
run -- vault set API_KEY
prompted "vault set value, loopback" "$P" "daemon /cli/vault/set"
run "${HTTPS[@]}" -- vault set API_KEY
prompted "vault set value, https" "$P"
# K2_PLAIN_HTTP_OK=1 and a tailnet host don't lift it.
run "${PLAIN[@]}" K2_PLAIN_HTTP_OK=1 -- vault set API_KEY
refused "vault set value, K2_PLAIN_HTTP_OK=1" 2 "$P" ""
WANT_SAVED="$WANT"
WANT="$(want_line "http://box.tail1234.ts.net:$DPORT")"
run K2_HOST=box.tail1234.ts.net -- vault set API_KEY
refused "vault set value, a tailnet host" 2 "$P" ""
WANT="$WANT_SAVED"

echo "== migrate.sh: a new vault password (vault confirm-password set) =="
P='New vault password'
scn '{"methods": ["terminal"]}'
TYPED_LINES=2
run "${PLAIN[@]}" -- vault confirm-password set
refused "new vault password" 2 "$P" ""
run -- vault confirm-password set
prompted "new vault password, loopback" "$P" "vault /cli/vault/confirm-password/set"
run "${HTTPS[@]}" -- vault confirm-password set
prompted "new vault password, https" "$P" "vault /cli/vault/confirm-password/set"
TYPED_LINES=1

echo "== migrate.sh: recovery words (vault restore) =="
P='Recovery words'
scn '{}'
run "${PLAIN[@]}" -- vault restore --from "$W/backup"
refused "recovery words" 2 "$P" ""
run -- vault restore --from "$W/backup"
prompted "recovery words, loopback" "$P" "vault /cli/vault/restore"
run "${HTTPS[@]}" -- vault restore --from "$W/backup"
prompted "recovery words, https" "$P" "vault /cli/vault/restore"

echo "== app-auth.sh: a relay password (notifications email relay add) =="
P='Relay password'
RELAY=(notifications email relay add --host smtp.example.test --username u1 --from-domain example.test)
run "${PLAIN[@]}" -- "${RELAY[@]}"
refused "relay password" 2 "$P" ""
run -- "${RELAY[@]}"
prompted "relay password, loopback" "$P" "daemon /cli/notifications/email/relay/add"
run "${HTTPS[@]}" -- "${RELAY[@]}"
prompted "relay password, https" "$P"

echo "== connect.sh: a site PIN (publish visibility ... pin) =="
P='PIN for acme'
TYPED_LINES=2
run "${PLAIN[@]}" -- publish visibility acme pin
refused "site PIN" 2 "$P" "daemon POST /cli/publish/visibility"
run -- publish visibility acme pin
prompted "site PIN, loopback" "$P" "daemon /cli/publish/visibility"
TYPED_LINES=1

echo "== llm.sh: an API token (llm tokens add-key) =="
P='Paste the API token: '
run "${PLAIN[@]}" -- llm tokens add-key claude work
refused "llm API token" 2 "$P" ""
run -- llm tokens add-key claude work
prompted "llm API token, loopback" "$P" "daemon /cli/llm/accounts/add-key"
run "${HTTPS[@]}" -- llm tokens add-key claude work
prompted "llm API token, https" "$P"

echo "== users-phones.sh: a login password (users pair-phone) =="
P='Password for your login: '
run "${PLAIN[@]}" -- users pair-phone
refused "login password" 2 "$P" "daemon POST /cli/auth/device/pair-code"
run -- users pair-phone
prompted "login password, loopback" "$P" "daemon /cli/auth/device/pair-code"

echo "== clone.sh: a login's password (users add, users set-password) =="
# getpass: with no terminal to open it reads the pty on stdin.
P='Password: '
TYPED_LINES=2
run "${PLAIN[@]}" -- users add bob
refused "users add password" 2 "$P" ""
# getpass flushes what was typed before it asks: type at each prompt.
GP="[[\"Password: \", \"$SECRET\\n\"], [\"Confirm password: \", \"$SECRET\\n\"]]"
STEPS="$GP"; run -- users add bob; STEPS=-
prompted "users add password, loopback" "$P" "daemon /cli/users/add"
STEPS="$GP"; run "${HTTPS[@]}" -- users add bob; STEPS=-
prompted "users add password, https" "$P"
run "${PLAIN[@]}" -- users set-password bob
refused "users set-password password" 2 "$P" ""
STEPS="$GP"; run -- users set-password bob; STEPS=-
prompted "users set-password password, loopback" "$P" "daemon /cli/users/set-password"
TYPED_LINES=1

echo "== talk-and-workspace.sh: LLM API keys (workspace api-key, api-key create) =="
P='Paste the API key: '
run "${PLAIN[@]}" -- workspace api-key ws1 --key-stdin
refused "workspace API key" 2 "$P" ""
run -- workspace api-key ws1 --key-stdin
prompted "workspace API key, loopback" "$P" "daemon /cli/workspace/api-key"
run "${HTTPS[@]}" -- workspace api-key ws1 --key-stdin
prompted "workspace API key, https" "$P"
P='Paste the LLM API key: '
run "${PLAIN[@]}" -- api-key create --label mon --key-stdin
refused "api-key LLM key" 2 "$P" ""
run -- api-key create --label mon --key-stdin
prompted "api-key LLM key, loopback" "$P" "daemon /cli/api-keys/create"
run "${HTTPS[@]}" -- api-key create --label mon --key-stdin
prompted "api-key LLM key, https" "$P"

echo
echo "password_prompt_plain_http: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
