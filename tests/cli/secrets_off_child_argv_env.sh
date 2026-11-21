#!/usr/bin/env bash
# Privacy check: a secret the CLI handles never lands in a child process's
# argv or environment. ps shows a command line to every account on this
# computer, and the environment to this user's processes, so neither is
# private. A confirm token is what keeps a person-only step person-only; it
# travels on stdin (or curl's config on stdin) only.
#
# The real CLI runs each command below against two stubs: one on the vault
# socket that answers every confirm with a sentinel token, and one on the
# daemon's TCP port that asks for that confirm (403 vault_confirm_required)
# and records each request. Every `python3` and `curl` the CLI starts is
# wrapped so its argv and its environment are logged. Then each sentinel
# must appear in the request the stub received (it really was sent) and in
# no child's argv and no child's environment.
#
# Covered: k2 vault backup key, k2 sources secret (the token and the
# secret's name), k2 app recording <app> set, k2 hostchat app-space
# recording, k2 hostchat app-space recording-lock, k2 hostchat recording
# policy set (the four callers of _rec_send_confirmed), k2 api-key create
# --hire, and the typed passwords
# of the mail family: k2 mail link add --pass-stdin, k2 mail oauth-config
# set --secret-stdin, k2 hostmail config --relay-pass-stdin, k2 hostmail
# import --pass-stdin, of App users (k2 app user add and password
# --password-stdin), the tunnel token k2 connect login binds, and of the
# logins, k2 tunnel switch --token-stdin, and the sign-in code pasted
# into k2 llm tokens add, and k2 fed pair-join --token-stdin: k2 users add and set-password
# --password-stdin, k2 users pair-phone, k2 connect login; and the remote
# server's token of k2 clone (--host, --token), and this server's own
# token in k2 clone's engine.
# Also the remote token of k2 read <ws>@<host> --token-stdin.
# Also the token of k2 daemon restart --host <url> --token-stdin.
# Also the remote token of k2 talk <ws>@<host> --token-stdin.
# Also the tokens of k2 remote-session status --host and write --token-stdin.
# Also the pairing code of k2 compute node servers add (--code -).
# Also the pairing code of k2 compute node servers add --code-stdin.
#
# No daemon build; a throwaway HOME with its own vault socket; never
# touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

TOKEN_SENTINEL="k2vc_CHILDTEST000sentinel"
SECRET_SENTINEL="CHILDTEST_SECRET_NAME"
PASS_SENTINEL="CHILDTEST_PASSWORD_7Kq"
REMOTE_SENTINEL="k2rt_CHILDTEST_remote_token"
TUNNEL_SENTINEL="k2tt_CHILDTEST_tunnel_token"
AGEKEY="age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p"

W="$(mktemp -d /tmp/k2child-XXXXXX)"
ARGLOG="$W/child-argv.log"
ENVLOG="$W/child-env.log"
TCPLOG="$W/tcp-requests.log"
: >"$ARGLOG"; : >"$ENVLOG"; : >"$TCPLOG"

cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    [ -n "${TCP_PID:-}" ] && kill "$TCP_PID" 2>/dev/null || true
    rm -rf "$W"
}
trap cleanup EXIT

# The real python3 and curl, found before we shadow them on PATH.
REAL_PY="$(command -v python3)"
REAL_CURL="$(command -v curl)" || { echo "FAIL: curl not found" >&2; exit 1; }
REAL_ENV="$(command -v env)"

# Wrappers that record their argv and their environment, then run the real
# program unchanged.
mkdir -p "$W/bin"
for prog in python3 curl; do
    real="$REAL_PY"; [ "$prog" = curl ] && real="$REAL_CURL"
    cat >"$W/bin/$prog" <<WRAP
#!/usr/bin/env bash
printf '$prog %s\n' "\$*" >>"$ARGLOG"
{ printf '== $prog\n'; "$REAL_ENV"; } >>"$ENVLOG"
exec "$real" "\$@"
WRAP
    chmod +x "$W/bin/$prog"
done
export PATH="$W/bin:$PATH"

# A throwaway HOME with a vault socket a stub answers on.
export HOME="$W/home"
mkdir -p "$HOME/.k2"
# Nothing may mark this a K2 session, or the person-only commands refuse
# before the confirm (every prefix _envn_any checks: K5_/K2_/K2SO_).
for p in HOOK_SOCK HOOK_TOKEN CELL PORT HOST REMOTE_TOKEN; do
    unset "K5_$p" "K2_$p" "K2SO_$p" 2>/dev/null || true
done
SOCK="$HOME/.k2/vault.sock"

# The vault stub: answers confirm/start with the vault-password method,
# confirm/finish with the sentinel token, and the use routes with ok,
# recording the last body it saw for each route.
"$REAL_PY" - "$SOCK" "$W" "$TOKEN_SENTINEL" <<'PYEOF' &
import json, os, socket, sys, threading
sock_path, work, token = sys.argv[1], sys.argv[2], sys.argv[3]
try:
    os.unlink(sock_path)
except FileNotFoundError:
    pass
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(sock_path)
srv.listen(16)
open(os.path.join(work, "ready"), "w").close()

def read_request(conn):
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = conn.recv(65536)
        if not chunk:
            return data, b""
        data += chunk
    head, _, rest = data.partition(b"\r\n\r\n")
    n = 0
    for line in head.split(b"\r\n")[1:]:
        k, _, v = line.partition(b":")
        if k.strip().lower() == b"content-length":
            n = int(v.strip())
    while len(rest) < n:
        chunk = conn.recv(65536)
        if not chunk:
            break
        rest += chunk
    return head, rest

def handle(conn):
    head, body = read_request(conn)
    line = head.split(b"\r\n", 1)[0].decode("latin1")
    parts = line.split(" ")
    path = parts[1].split("?")[0] if len(parts) > 1 else ""
    route = path.rsplit("/cli/vault/", 1)[-1]
    with open(os.path.join(work, "req-" + route.replace("/", "_")), "wb") as f:
        f.write(body)
    if route.endswith("confirm/start"):
        out = {"methods": ["vault-password"], "expiresIn": 60}
    elif route.endswith("confirm/finish"):
        out = {"token": token, "expiresIn": 60}
    elif route == "status":
        out = {"open": True, "recovery": "confirmed", "confirmPassword": True}
    else:
        out = {"ok": True, "fingerprint": "fp", "name": "x", "secret": "x", "secretBound": True, "names": ["x"], "path": work, "note": "ok"}
    payload = json.dumps(out).encode()
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s" % (len(payload), payload))
    conn.close()

while True:
    try:
        c, _ = srv.accept()
    except OSError:
        break
    threading.Thread(target=handle, args=(c,), daemon=True).start()
PYEOF
STUB_PID=$!

# The daemon stub on TCP: logs every request as one JSON line (method,
# path, Authorization header, body). A recording change without a
# confirmToken gets 403 vault_confirm_required, as the daemon answers a
# widening; with one it is saved.
"$REAL_PY" - "$W/tcp-port" "$TCPLOG" "$TUNNEL_SENTINEL" <<'PYEOF' &
import http.server, json, sys, threading
port_path, log_path, tunnel_token = sys.argv[1], sys.argv[2], sys.argv[3]
lock = threading.Lock()
LLM = {"code_in": False}
RECORDING = {
    "/cli/app/recording/set": "recording-app",
    "/cli/sew/app-spaces/recording": "recording-space",
    "/cli/sew/app-spaces/recording-lock": "recording-lock",
    "/cli/sew/recording/policy": "recording-policy",
}
class H(http.server.BaseHTTPRequestHandler):
    def _answer(self, status, obj):
        out = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        path = self.path.split("?")[0]
        with lock, open(log_path, "a") as f:
            f.write(json.dumps({"method": self.command, "path": path,
                                "auth": self.headers.get("Authorization", ""), "body": raw}) + "\n")
        try:
            body = json.loads(raw) if raw.strip() else {}
        except ValueError:
            body = {}
        if self.command == "POST" and path in RECORDING:
            if not body.get("confirmToken"):
                return self._answer(403, {"error": {"code": "vault_confirm_required", "hint": "confirm in person",
                                                    "action": RECORDING[path], "serviceId": "svc-1"}})
            return self._answer(200, {"changed": True, "app": {"name": "acme", "serviceId": "svc-1"}, "flags": {},
                                      "recording": {}, "spaces": [], "spaceId": "5", "locked": False, "policies": {}})
        if path == "/cli/app/recording":
            return self._answer(200, {"app": {"name": "acme", "serviceId": "svc-1"}, "flags": {}, "recording": {}, "spaces": []})
        if path == "/cli/sew/status":
            return self._answer(200, {"installId": "inst-1"})
        if path == "/cli/sources/show":
            return self._answer(200, {"source": {"id": "src-1", "name": "a-source"}})
        if path == "/cli/sources/secret":
            return self._answer(200, {"names": ["a-source"], "secret": "x", "secretBound": True})
        if path == "/cli/auth/device/pair-code":
            if body.get("password"):
                return self._answer(200, {"code": "123456", "expiresIn": 300})
            return self._answer(200, {"error": {"code": "password_required",
                                                "hint": "type your password again to make a pairing code"}})
        if path == "/cli/llm/accounts/add":
            LLM["code_in"] = False
            return self._answer(200, {"login": {"loginId": "L1"}})
        if path == "/cli/llm/accounts/login/input":
            LLM["code_in"] = True
            return self._answer(200, {"ok": True})
        if path == "/cli/llm/accounts/login/status":
            if LLM["code_in"]:
                return self._answer(200, {"login": {"state": "signed_in", "done": True, "tool": "claude", "label": "work"}})
            return self._answer(200, {"login": {"state": "waiting_for_code"}})
        if path == "/rest/v1/subdomains":
            return self._answer(200, [{"label": "acme", "status": "active", "tunnel_token": tunnel_token}])
        if path == "/rest/v1/rpc/claim_subdomain":
            return self._answer(200, [{"claimed": True}])
        if path == "/cli/tunnel/config":
            return self._answer(200, {"tokenSet": True, "subdomain": "acme"})
        if path == "/auth/v1/token":
            return self._answer(401, {"msg": "Invalid login credentials"})
        if path == "/cli/clone/preview":
            return self._answer(200, {"workspace": "ws1", "secrets": {"rows": []}, "callerCanCopySecrets": True})
        if path == "/cli/compute/nodes/servers/add":
            return self._answer(200, {"request": {"id": "req-1", "summary": "add a server"}})
        if path == "/cli/daemon/restart":
            return self._answer(200, {"restarting": True})
        if path == "/cli/api-keys/create":
            return self._answer(200, {"id": "k1", "key": "k2ak_test", "capabilities": {"hire": True}})
        return self._answer(200, {"ok": True})
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(port_path + ".tmp", "w") as f:
    f.write(str(srv.server_address[1]))
import os
os.rename(port_path + ".tmp", port_path)
srv.serve_forever()
PYEOF
TCP_PID=$!
for _ in $(seq 1 50); do [ -f "$W/ready" ] && [ -s "$W/tcp-port" ] && break; sleep 0.1; done
[ -f "$W/ready" ] || { echo "FAIL: vault stub never came up" >&2; exit 1; }
[ -s "$W/tcp-port" ] || { echo "FAIL: tcp stub never came up" >&2; exit 1; }

# The CLI bootstrap finds the daemon through these files (a real terminal
# outside K2 does the same). No K2_HOOK_* env, so the person check still
# sees a terminal, not a session.
cat "$W/tcp-port" >"$HOME/.k2/heartbeat.port"
printf 'test-token\n' >"$HOME/.k2/heartbeat.token"
# k2 fed reads the owner token from daemon.token (a fresh server has both).
printf 'test-token\n' >"$HOME/.k2/daemon.token"
cd "$W"

PASS=0; FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

fresh() { : >"$ARGLOG"; : >"$ENVLOG"; : >"$TCPLOG"; rm -f "$W"/req-*; }

# The wrappers really ran: at least one python3 child (and curl when the
# command talks TCP), else every "not in argv" below would pass vacuously.
assert_children_seen() {
    local label="$1" need_curl="${2:-}"
    if grep -q '^python3 ' "$ARGLOG"; then ok "$label: python3 children were logged"; else bad "$label: no python3 child was logged"; fi
    if [ -n "$need_curl" ]; then
        if grep -q '^curl ' "$ARGLOG"; then ok "$label: curl children were logged"; else bad "$label: no curl child was logged"; fi
    fi
}

# assert_children_native LABEL [curl]: for a command the k5 binary
# answers itself, the binary under test (K5_CLI_UNDER_TEST) starts no
# python3 and no curl at all; the bash CLI's python3 children (and curl's,
# with curl) are logged as above.
assert_children_native() {
    local label="$1" need_curl="${2:-}"
    if [ -z "${K5_CLI_UNDER_TEST:-}" ]; then
        assert_children_seen "$label" "$need_curl"
    elif grep -Eq '^(python3|curl) ' "$ARGLOG"; then
        bad "$label: the binary started a python3 or curl child"
    else
        ok "$label: the binary started no python3 or curl child"
    fi
}

# assert_off_children LABEL WHAT VALUE: VALUE is in no child's argv and no
# child's environment.
assert_off_children() {
    local label="$1" what="$2" value="$3"
    if grep -Fq "$value" "$ARGLOG"; then
        bad "$label: the $what is in a child's argv"
        grep -F "$value" "$ARGLOG" | head -3 >&2
    else
        ok "$label: no $what in any child's argv"
    fi
    if grep -Fq "$value" "$ENVLOG"; then
        bad "$label: the $what is in a child's environment"
        grep -F "$value" "$ENVLOG" | head -3 >&2
    else
        ok "$label: no $what in any child's environment"
    fi
}

# assert_tcp_got LABEL PATH WHAT VALUE: the daemon stub got a POST to PATH
# whose body or Authorization header carries VALUE.
assert_tcp_got() {
    local label="$1" path="$2" what="$3" value="$4"
    if "$REAL_PY" -c '
import json, sys
path, value = sys.argv[2], sys.argv[3]
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
sys.exit(0 if any(r["path"] == path and (value in r["body"] or value in r["auth"]) for r in rows) else 1)
' "$TCPLOG" "$path" "$value"; then
        ok "$label: the daemon received the $what"
    else
        bad "$label: the daemon never received the $what on $path"
        cut -c1-200 "$TCPLOG" >&2
    fi
}

# 1. k2 vault backup key <age1…>: the confirm token rides stdin.
fresh
printf '\n' | "$K2_CLI" vault backup key "$AGEKEY" >/dev/null 2>&1 || true
if [ -f "$W/req-backup_recipient" ] && grep -Fq "$TOKEN_SENTINEL" "$W/req-backup_recipient"; then
    ok "backup key: the daemon received the confirm token"
else
    bad "backup key: the daemon never received the confirm token"
fi
# The k5 binary answers vault backup key itself: it starts no child, so
# the guard is that the daemon got the token (above) and no child ran.
if [ -n "${K5_CLI_UNDER_TEST:-}" ]; then
    if [ -s "$ARGLOG" ]; then bad "backup key: the binary started a child"; head -3 "$ARGLOG" >&2; else ok "backup key: the binary started no child"; fi
else
    assert_children_seen "backup key" curl
fi
assert_off_children "backup key" "confirm token" "$TOKEN_SENTINEL"

# 2. k2 sources secret <name> <secret>: the token and the secret's name
#    ride stdin.
fresh
printf '\n' | "$K2_CLI" sources secret "a-source" "$SECRET_SENTINEL" >/dev/null 2>&1 || true
assert_tcp_got "sources secret" /cli/sources/secret "confirm token" "$TOKEN_SENTINEL"
assert_tcp_got "sources secret" /cli/sources/secret "secret's name" "$SECRET_SENTINEL"
assert_children_native "sources secret" curl
assert_off_children "sources secret" "confirm token" "$TOKEN_SENTINEL"
assert_off_children "sources secret" "secret's name" "$SECRET_SENTINEL"

# 3-5b. The four callers of _rec_send_confirmed: the stub asks for the
#       confirm, the CLI gets it and sends the change again with the token.
#       rec_case_native: each is a command the k5 binary answers itself
#       (app recording and hostchat), which starts no python3 or curl child.
rec_case_native() {
    local label="$1" path="$2"; shift 2
    fresh
    printf '\n' | "$K2_CLI" "$@" >/dev/null 2>&1 || true
    assert_tcp_got "$label" "$path" "confirm token" "$TOKEN_SENTINEL"
    assert_children_native "$label" curl
    assert_off_children "$label" "confirm token" "$TOKEN_SENTINEL"
}
rec_case_native "app recording set" /cli/app/recording/set app recording acme set --start-level admin
rec_case_native "app-space recording" /cli/sew/app-spaces/recording hostchat app-space recording acme 5 --start-level admin
rec_case_native "app-space recording-lock" /cli/sew/app-spaces/recording-lock hostchat app-space recording-lock 5 off
rec_case_native "recording policy set" /cli/sew/recording/policy hostchat recording policy set audio_max_days 60

# 6. k2 api-key create --hire: the hire key's confirm token joins the body
#    on stdin.
fresh
printf '\n' | "$K2_CLI" api-key create --label hire-test --workspaces ws1 --hire >/dev/null 2>&1 || true
assert_tcp_got "api-key create --hire" /cli/api-keys/create "confirm token" "$TOKEN_SENTINEL"
assert_children_native "api-key create --hire" curl
assert_off_children "api-key create --hire" "confirm token" "$TOKEN_SENTINEL"

# 7-10. The mail family's passwords and OAuth secret: typed on stdin, they
#       reach the daemon in the body and no child's argv or environment.
pw_case() {
    local label="$1" path="$2"; shift 2
    fresh
    printf '%s\n' "$PASS_SENTINEL" | "$K2_CLI" "$@" >/dev/null 2>&1 || true
    assert_tcp_got "$label" "$path" "password" "$PASS_SENTINEL"
    assert_children_seen "$label"
    assert_off_children "$label" "password" "$PASS_SENTINEL"
}
# pw_case_native: the same, for a command the k5 binary answers itself
# (it starts no python3; the bash CLI's children are logged as above).
pw_case_native() {
    local label="$1" path="$2"; shift 2
    fresh
    printf '%s\n' "$PASS_SENTINEL" | "$K2_CLI" "$@" >/dev/null 2>&1 || true
    assert_tcp_got "$label" "$path" "password" "$PASS_SENTINEL"
    assert_children_native "$label"
    assert_off_children "$label" "password" "$PASS_SENTINEL"
}
pw_case_native "mail link add" /cli/mail/external/add mail link add a@example.test --host imap.example.test --workspace ws1 --pass-stdin --json
pw_case_native "mail oauth-config set" /cli/mail/oauth-config/set mail oauth-config set --provider gmail --client-id cid --secret-stdin --json
pw_case_native "hostmail config relay" /cli/mail/config/set hostmail config --relay-pass-stdin --json
pw_case_native "hostmail import" /cli/mail/import hostmail import --address a@example.test --imapsync from=imap.example.test --user u --pass-stdin --json

# 11-14. Login passwords: typed on stdin, they reach the request body and
#        no child's argv or environment.
pw_case_native "users add" /cli/users/add users add alice --password-stdin
pw_case_native "users set-password" /cli/users/set-password users set-password alice --password-stdin
pw_case_native "users pair-phone" /cli/auth/device/pair-code users pair-phone
# k2 connect login asks with getpass, which reads stdin when there is no
# terminal: setsid drops the controlling terminal so the test can type.
# (The k5 binary reads it there itself, with its own warning line: the
# named difference getpass-no-tty-warning. So it starts no child.)
SETSID="$(command -v setsid)" || { echo "FAIL: setsid not found (util-linux)" >&2; exit 1; }
fresh
printf '%s\n' "$PASS_SENTINEL" | K2_CONNECT_DB_URL="http://127.0.0.1:$(cat "$W/tcp-port")/" \
    "$SETSID" -w "$K2_CLI" connect login --email a@example.test --subdomain acme >/dev/null 2>&1 || true
assert_tcp_got "connect login" /auth/v1/token "password" "$PASS_SENTINEL"
assert_children_native "connect login"
assert_off_children "connect login" "password" "$PASS_SENTINEL"

# sec_case LABEL PATH WHAT VALUE ARGS…: VALUE typed on stdin reaches PATH
# and no child's argv or environment, nor k2's own argv (logged by k2run,
# so a secret put on the command line counts as leaked too).
k2run() { printf 'k2 %s\n' "$*" >>"$ARGLOG"; "$K2_CLI" "$@"; }
sec_case() {
    local label="$1" path="$2" what="$3" value="$4"; shift 4
    fresh
    printf '%s\n' "$value" | k2run "$@" >/dev/null 2>&1 || true
    assert_tcp_got "$label" "$path" "$what" "$value"
    "${SEC_CASE_CHILDREN:-assert_children_seen}" "$label"
    assert_off_children "$label" "$what" "$value"
}

# 15. k2 clone --host … --token: the remote server's token reaches that
#     server's request and no child's argv or environment.
fresh
"$K2_CLI" clone preview ws1 --host "http://127.0.0.1:$(cat "$W/tcp-port")" --token "$REMOTE_SENTINEL" --json </dev/null >/dev/null 2>&1 || true
assert_tcp_got "clone preview --host" /cli/clone/preview "remote token" "$REMOTE_SENTINEL"
assert_children_native "clone preview --host"
assert_off_children "clone preview --host" "remote token" "$REMOTE_SENTINEL"

# 15b. k2 clone preview on this server: this server's own token reaches its
#      request and no child's argv or environment (it rides the engine's
#      stdin with the remote token, not its environment).
LOCAL_SENTINEL="k2lt_CHILDTEST_local_token"
fresh
printf '%s\n' "$LOCAL_SENTINEL" >"$HOME/.k2/heartbeat.token"
printf '%s\n' "$LOCAL_SENTINEL" >"$HOME/.k2/daemon.token"
"$K2_CLI" clone preview ws1 --json </dev/null >/dev/null 2>&1 || true
printf 'test-token\n' >"$HOME/.k2/heartbeat.token"
printf 'test-token\n' >"$HOME/.k2/daemon.token"
assert_tcp_got "clone preview (this server)" /cli/clone/preview "local token" "$LOCAL_SENTINEL"
assert_children_native "clone preview (this server)"
assert_off_children "clone preview (this server)" "local token" "$LOCAL_SENTINEL"

# 15c. k2 clone --host … --token-stdin: the remote server's token reaches
#      that server's request and no child's argv or environment, nor k2's
#      own argv.
SEC_CASE_CHILDREN=assert_children_native sec_case "clone preview --token-stdin" /cli/clone/preview "remote token" "$REMOTE_SENTINEL" clone preview ws1 --host "http://127.0.0.1:$(cat "$W/tcp-port")" --token-stdin --json

# 16-17. An App user's password (k2 app user add / password --password-stdin).
SEC_CASE_CHILDREN=assert_children_native sec_case "app user add" /cli/skin/users "password" "$PASS_SENTINEL" app user add bob --password-stdin
SEC_CASE_CHILDREN=assert_children_native sec_case "app user password" /cli/skin/users/password "password" "$PASS_SENTINEL" app user password bob --password-stdin

# 18. k2 connect login: the subdomain's tunnel token, read from the account
#     server, reaches the daemon's tunnel config and no child's argv or
#     environment.
fresh
K2_CONNECT_DB_URL="http://127.0.0.1:$(cat "$W/tcp-port")/" \
    k2run connect login --token test-access --subdomain acme </dev/null >/dev/null 2>&1 || true
assert_tcp_got "connect login tunnel" /cli/tunnel/config "tunnel token" "$TUNNEL_SENTINEL"
assert_children_native "connect login tunnel"
assert_off_children "connect login tunnel" "tunnel token" "$TUNNEL_SENTINEL"

# 19. k2 tunnel switch <sub> --token-stdin: the tunnel token reaches the
#     switch and no child's argv or environment, nor k2's own argv.
fresh
printf '%s\n' "$TUNNEL_SENTINEL" | k2run tunnel switch acme --token-stdin >/dev/null 2>&1 || true
assert_tcp_got "tunnel switch --token-stdin" /cli/tunnel/switch "tunnel token" "$TUNNEL_SENTINEL"
assert_children_native "tunnel switch --token-stdin"
assert_off_children "tunnel switch --token-stdin" "tunnel token" "$TUNNEL_SENTINEL"

# 20. k2 llm tokens add: the sign-in code pasted on stdin reaches
#     login/input and no child's argv or environment. llm is native in
#     the k5 binary, which starts no child at all.
K2_LLMA_POLL_SECS=0 SEC_CASE_CHILDREN=assert_children_native sec_case "llm tokens add (code)" /cli/llm/accounts/login/input "sign-in code" "$PASS_SENTINEL" llm tokens add claude work

# 21. k2 fed pair-join --token-stdin: the other owner's pairing token
#     reaches the pairing route (decoded) and, as typed, no child's argv
#     or environment, nor k2's own argv.
FED_TOKEN="k2pair1:$("$REAL_PY" -c 'import base64,json,sys; print(base64.urlsafe_b64encode(json.dumps({"n": sys.argv[1]}).encode()).decode().rstrip("="))' "$PASS_SENTINEL")"
fresh
printf '%s\n' "$FED_TOKEN" | k2run fed pair-join --token-stdin >/dev/null 2>&1 || true
assert_tcp_got "fed pair-join --token-stdin" /cli/federation/pair/v2/respond "pairing token" "$PASS_SENTINEL"
assert_children_native "fed pair-join --token-stdin" curl
assert_off_children "fed pair-join --token-stdin" "pairing token" "$FED_TOKEN"

# 22. k2 read <ws>@<host> --token-stdin: the remote server's token reaches
#     that server's terminal read and no child's argv or environment, nor
#     k2's own argv.
SEC_CASE_CHILDREN=assert_children_native sec_case "read --token-stdin" /cli/terminal/read "remote token" "$REMOTE_SENTINEL" read "ws1@http://127.0.0.1:$(cat "$W/tcp-port")" --token-stdin

# 23. k2 daemon restart --host <url> --token-stdin: the token reaches that
#     daemon's restart route and no child's argv or environment, nor k2's
#     own argv. The remote restart needs only curl, so this case checks
#     for curl children itself. The k5 binary answers it itself: no
#     python3 or curl child at all.
fresh
printf '%s\n' "$REMOTE_SENTINEL" | k2run daemon restart --host "http://127.0.0.1:$(cat "$W/tcp-port")" --token-stdin >/dev/null 2>&1 || true
assert_tcp_got "daemon restart --token-stdin" /cli/daemon/restart "token" "$REMOTE_SENTINEL"
if [ -n "${K5_CLI_UNDER_TEST:-}" ]; then
    if grep -Eq '^(python3|curl) ' "$ARGLOG"; then bad "daemon restart --token-stdin: the binary started a python3 or curl child"; else ok "daemon restart --token-stdin: the binary started no python3 or curl child"; fi
elif grep -q '^curl ' "$ARGLOG"; then ok "daemon restart --token-stdin: curl children were logged"; else bad "daemon restart --token-stdin: no curl child was logged"; fi
assert_off_children "daemon restart --token-stdin" "token" "$REMOTE_SENTINEL"

# 24. k2 talk <ws>@<host> "text" --token-stdin: the remote server's token
#     reaches that server's terminal read (talk reads first) and no
#     child's argv or environment, nor k2's own argv, through the send.
#     The k5 binary answers talk itself: no python3 or curl child at all.
fresh
printf '%s\n' "$REMOTE_SENTINEL" | k2run talk "ws1@http://127.0.0.1:$(cat "$W/tcp-port")" "hello" --token-stdin >/dev/null 2>&1 || true
assert_tcp_got "talk --token-stdin" /cli/terminal/read "remote token" "$REMOTE_SENTINEL"
assert_children_native "talk --token-stdin" curl
assert_off_children "talk --token-stdin" "remote token" "$REMOTE_SENTINEL"

# 25-26. k2 remote-session --token-stdin: an owner token for a remote host
#        (status --host) and a grant token on this daemon (write) reach the
#        route and no child's argv or environment, nor k2's own argv.
SEC_CASE_CHILDREN=assert_children_native sec_case "remote-session status --token-stdin" /cli/remote-session/status "owner token" "$REMOTE_SENTINEL" remote-session status --host "http://127.0.0.1:$(cat "$W/tcp-port")" --token-stdin
SEC_CASE_CHILDREN=assert_children_native sec_case "remote-session write --token-stdin" /cli/terminal/write "grant token" "$REMOTE_SENTINEL" remote-session write sess-1 "ls" --token-stdin

# 27. k2 compute node servers <node> add --code -: the pairing code reaches
#     the add route and no child's argv or environment, nor k2's own argv.
SEC_CASE_CHILDREN=assert_children_native sec_case "compute node servers add --code -" /cli/compute/nodes/servers/add "pairing code" "$PASS_SENTINEL" compute node servers node-1 add --url https://globex.example --code -

# 28. k2 compute node servers <node> add --code-stdin: the pairing code
#     reaches the add route and no child's argv or environment, nor k2's
#     own argv.
SEC_CASE_CHILDREN=assert_children_native sec_case "compute node servers add --code-stdin" /cli/compute/nodes/servers/add "pairing code" "$PASS_SENTINEL" compute node servers node-1 add --url https://globex.example --code-stdin

echo "secrets_off_child_argv_env: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
