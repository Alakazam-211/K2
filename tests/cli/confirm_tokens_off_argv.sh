#!/usr/bin/env bash
# Privacy check: the confirm token and the secret name the CLI handles
# never land in a child process's argv (on Linux every user can read
# /proc/<pid>/cmdline, so argv is not private). The real cli/k2 runs
# `k2 vault backup key <age1…>` and `k2 sources secret <name> <secret>`
# against a stub that answers each confirm with a sentinel token. Every
# `python3` and `curl` the CLI starts is wrapped so its argv is logged;
# then the sentinel token and the secret must appear in the request the
# stub received (they really were sent) but in no child's argv.
#
# No daemon build; a throwaway HOME with its own vault socket; never
# touches the real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"
[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

TOKEN_SENTINEL="k2vc_ARGVTEST000sentinel"
SECRET_SENTINEL="ARGVTEST_SECRET_NAME"
AGEKEY="age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p"

W="$(mktemp -d /tmp/k2argv-XXXXXX)"
ARGLOG="$W/child-argv.log"
: >"$ARGLOG"

cleanup() {
    [ -n "${STUB_PID:-}" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$W"
}
trap cleanup EXIT

# The real python3 and curl, found before we shadow them on PATH.
REAL_PY="$(command -v python3)"
REAL_CURL="$(command -v curl || true)"

# Wrappers that record their argv, then run the real program unchanged.
mkdir -p "$W/bin"
cat >"$W/bin/python3" <<WRAP
#!/usr/bin/env bash
printf 'python3 %s\n' "\$*" >>"$ARGLOG"
exec "$REAL_PY" "\$@"
WRAP
if [ -n "$REAL_CURL" ]; then
    cat >"$W/bin/curl" <<WRAP
#!/usr/bin/env bash
printf 'curl %s\n' "\$*" >>"$ARGLOG"
exec "$REAL_CURL" "\$@"
WRAP
    chmod +x "$W/bin/curl"
fi
chmod +x "$W/bin/python3"
export PATH="$W/bin:$PATH"

# A throwaway HOME with a vault socket a stub answers on.
export HOME="$W/home"
mkdir -p "$HOME/.k2"
# Nothing may mark this a K2 session, or the person-only vault commands
# refuse before the confirm (every prefix _envn_any checks: K5_/K2_/K2SO_).
for p in HOOK_SOCK HOOK_TOKEN CELL; do
    unset "K5_$p" "K2_$p" "K2SO_$p" 2>/dev/null || true
done
unset K5_PORT K2_PORT K2SO_PORT K5_HOST K2_HOST K2SO_HOST 2>/dev/null || true
# The CLI bootstrap finds the daemon through these files (a real terminal
# outside K2 does the same); the vault calls themselves go over the socket.
# No K2_HOOK_* env, so the person check still sees a terminal, not a session.
printf '1
' >"$HOME/.k2/heartbeat.port"
printf 'test-token
' >"$HOME/.k2/heartbeat.token"
SOCK="$HOME/.k2/vault.sock"

# The stub daemon on the vault socket: answers confirm/start with the
# sentinel token, confirm/finish by echoing it, and the use routes with
# ok, recording the last body it saw for each route.
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

def body_of(data):
    head, _, rest = data.partition(b"\r\n\r\n")
    return rest

def handle(conn):
    data = conn.recv(65536)
    line = data.split(b"\r\n", 1)[0].decode("latin1")
    path = line.split(" ")[1].split("?")[0] if len(line.split(" ")) > 1 else ""
    route = path.rsplit("/cli/vault/", 1)[-1]
    with open(os.path.join(work, "req-" + route.replace("/", "_")), "wb") as f:
        f.write(body_of(data))
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
for _ in $(seq 1 50); do [ -f "$W/ready" ] && break; sleep 0.1; done
[ -f "$W/ready" ] || { echo "FAIL: stub never came up" >&2; exit 1; }

PASS=0; FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

# A confirm token never appears in a child's argv.
assert_no_token_in_argv() {
    if grep -Fq "$TOKEN_SENTINEL" "$ARGLOG"; then
        bad "$1: the confirm token is in a child's argv"
        grep -F "$TOKEN_SENTINEL" "$ARGLOG" | head -3 >&2
    else
        ok "$1: no confirm token in any child's argv"
    fi
}

# 1. k2 vault backup key <age1…>: the confirm token rides stdin.
: >"$ARGLOG"
printf '\n' | "$K2_CLI" vault backup key "$AGEKEY" >/dev/null 2>&1 || true
if [ -f "$W/req-backup_recipient" ] && grep -Fq "$TOKEN_SENTINEL" "$W/req-backup_recipient"; then
    ok "backup key: the daemon received the confirm token"
else
    bad "backup key: the daemon never received the confirm token"
fi
assert_no_token_in_argv "backup key"

# 2. k2 sources secret <name> <secret>: the token and the secret ride stdin.
: >"$ARGLOG"
"$K2_CLI" sources secret "a-source" "$SECRET_SENTINEL" >/dev/null 2>&1 || true
if grep -Fq "$SECRET_SENTINEL" "$ARGLOG"; then
    bad "sources secret: the secret is in a child's argv"
    grep -F "$SECRET_SENTINEL" "$ARGLOG" | head -3 >&2
else
    ok "sources secret: no secret in any child's argv"
fi
assert_no_token_in_argv "sources secret"

echo "confirm_tokens_off_argv: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
