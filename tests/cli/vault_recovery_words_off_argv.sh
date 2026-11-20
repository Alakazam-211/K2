#!/usr/bin/env bash
# Privacy check: the recovery words typed back to k2 vault recovery show-once
# and k2 vault recovery rotate never land in a child process's argv or
# environment (ps shows a command line to every user on this computer, and the
# environment to this user's processes).
# They reach the confirm request on a pipe only.
#
# The real CLI runs against a stub on the vault socket that shows a
# 24-word phrase, records each request, and for rotate asks the vault
# password confirm. Every python3, curl and env the CLI starts is wrapped
# so its argv and its environment are logged, and k2's own argv is logged
# too. The typed words (and the shown phrase, which holds them) must reach
# the confirm body and appear in no logged argv and no logged environment.
# The output and exit codes are checked as well.
#
# No daemon build; a throwaway HOME; never touches the real ~/.k2.
# Runs with the bash 3.2 of macOS as well.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

W3="RECWORD3_CHILDTEST"
W11="RECWORD11_CHILDTEST"
W20="RECWORD20_CHILDTEST"
VAULT_PW="RECVAULTPW_CHILDTEST"
TOKEN="k2vc_RECCHILDTEST_token"

# /tmp, not $TMPDIR: a unix socket path must stay short on macOS.
W="$(mktemp -d /tmp/k2rec-XXXXXX)"
ARGLOG="$W/child-argv.log"
ENVLOG="$W/child-env.log"
REQLOG="$W/vault-requests.log"
: >"$ARGLOG"; : >"$ENVLOG"; : >"$REQLOG"

cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$W"
}
trap cleanup EXIT

REAL_PY="$(command -v python3)" || { echo "FAIL: python3 not found" >&2; exit 1; }
REAL_CURL="$(command -v curl)" || { echo "FAIL: curl not found" >&2; exit 1; }
REAL_ENV="$(command -v env)" || { echo "FAIL: env not found" >&2; exit 1; }

# Wrappers that record their argv and their environment, then run the real
# program unchanged.
mkdir -p "$W/bin"
for prog in python3 curl env; do
    case "$prog" in
        python3) real="$REAL_PY" ;;
        curl) real="$REAL_CURL" ;;
        env) real="$REAL_ENV" ;;
    esac
    cat >"$W/bin/$prog" <<WRAP
#!/usr/bin/env bash
printf '$prog %s\n' "\$*" >>"$ARGLOG"
{ printf '== $prog\n'; "$REAL_ENV"; } >>"$ENVLOG"
exec "$real" "\$@"
WRAP
    chmod +x "$W/bin/$prog"
done
export PATH="$W/bin:$PATH"

# A throwaway HOME. Nothing may mark this a K2 session (test process only),
# or the person-only verbs refuse before anything is asked.
export HOME="$W/home"
mkdir -p "$HOME/.k2"
for p in HOOK_SOCK HOOK_TOKEN CELL PORT HOST REMOTE_TOKEN HOME VAULT_CONFIRM_METHOD; do
    unset "K5_$p" "K2_$p" "K2SO_$p" 2>/dev/null || true
done
printf '1\n' >"$HOME/.k2/heartbeat.port"
printf 'test-token\n' >"$HOME/.k2/heartbeat.token"
SOCK="$HOME/.k2/vault.sock"

# The vault stub: status says the recovery key is confirmed, so show-once
# needs no confirm and rotate asks the vault password; show-once and rotate
# answer a phrase that holds the three words at 3, 11 and 20.
"$REAL_PY" - "$SOCK" "$W" "$REQLOG" "$TOKEN" "$W3" "$W11" "$W20" <<'PYEOF' &
import json, os, socket, sys, threading
sock_path, work, log_path, token, w3, w11, w20 = sys.argv[1:8]
words = ["w%02d" % i for i in range(1, 25)]
words[2], words[10], words[19] = w3, w11, w20
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(sock_path)
srv.listen(16)
open(os.path.join(work, "ready"), "w").close()
lock = threading.Lock()

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
    parts = head.split(b"\r\n", 1)[0].decode("latin1").split(" ")
    route = parts[1].split("?")[0].rsplit("/cli/vault/", 1)[-1] if len(parts) > 1 else ""
    with lock, open(log_path, "a") as f:
        f.write(json.dumps({"method": parts[0], "route": route, "body": body.decode("utf-8", "replace")}) + "\n")
    status = 200
    if route == "status":
        out = {"open": True, "recovery": "confirmed"}
    elif route == "confirm/start":
        out = {"methods": ["vault-password"], "expiresIn": 60}
    elif route == "confirm/finish":
        out = {"token": token, "expiresIn": 60}
    elif route in ("recovery/show-once", "recovery/rotate"):
        out = {"id": "rec-1", "phrase": " ".join(words)}
    elif route == "recovery/confirm":
        out = {"confirmed": True}
    else:
        status, out = 404, {"error": {"code": "not_found", "hint": "the stub answers the recovery routes only"}}
    payload = json.dumps(out).encode()
    conn.sendall(b"HTTP/1.1 %d X\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s" % (status, len(payload), payload))
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
[ -f "$W/ready" ] || { echo "FAIL: the vault stub never came up" >&2; exit 1; }
cd "$W"

PASS=0; FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
expect_eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi
}
fresh() { : >"$ARGLOG"; : >"$ENVLOG"; : >"$REQLOG"; }

# run_k2 STDIN ARGS...: runs k2 with STDIN on its stdin; sets OUT and RC.
run_k2() {
    local input="$1"; shift
    printf 'k2 %s\n' "$*" >>"$ARGLOG"
    RC=0
    OUT="$(printf '%s' "$input" | "$K2_CLI" "$@" 2>"$W/stderr")" || RC=$?
}

assert_off_children() {
    local label="$1" what="$2" value="$3"
    if grep -Fq "$value" "$ARGLOG"; then
        bad "$label: $what is in a child's argv"
        grep -F "$value" "$ARGLOG" | cut -c1-200 | head -3 >&2
    else
        ok "$label: $what is in no child's argv"
    fi
    if grep -Fq "$value" "$ENVLOG"; then
        bad "$label: $what is in a child's environment"
    else
        ok "$label: $what is in no child's environment"
    fi
}

# assert_routes LABEL ROUTE...: the stub got exactly these requests, in order.
assert_routes() {
    local label="$1"; shift
    local got want
    got="$("$REAL_PY" -c 'import json,sys; print(" ".join(json.loads(l)["route"] for l in open(sys.argv[1]) if l.strip()))' "$REQLOG")"
    want="$*"
    expect_eq "$label: the requests" "$got" "$want"
}

assert_confirm_body() {
    local label="$1"
    if "$REAL_PY" -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
c = [r for r in rows if r["route"] == "recovery/confirm"]
if len(c) != 1:
    sys.exit("%d confirm requests" % len(c))
got = json.loads(c[0]["body"])
want = {"id": "rec-1", "words": sys.argv[2:5]}
if got != want:
    sys.exit("confirm body %r, want %r" % (got, want))
' "$REQLOG" "$W3" "$W11" "$W20" 2>"$W/body-err"; then
        ok "$label: the confirm body carries the three typed words"
    else
        bad "$label: $(cat "$W/body-err")"
    fi
}

CONFIRMED="recovery key confirmed. If it ever leaks: k2 vault recovery rotate, then k2 vault rekey."

# 1. show-once: the three typed words go to the confirm request.
fresh
run_k2 "$W3"$'\n'"$W11"$'\n'"$W20"$'\n' vault recovery show-once
expect_eq "show-once: exit 0" "$RC" "0"
expect_eq "show-once: output" "$OUT" "$CONFIRMED"
assert_routes "show-once" status recovery/show-once recovery/confirm
assert_confirm_body "show-once"
if grep -q '^python3 ' "$ARGLOG" && grep -q '^curl ' "$ARGLOG"; then ok "show-once: python3 and curl children were logged"; else bad "show-once: the wrappers saw no python3 or curl child"; fi
for w in "$W3" "$W11" "$W20"; do assert_off_children "show-once" "the word $w" "$w"; done

# 2. rotate: the vault password confirm first, then the words.
fresh
run_k2 "$VAULT_PW"$'\n'"$W3"$'\n'"$W11"$'\n'"$W20"$'\n' vault recovery rotate
expect_eq "rotate: exit 0" "$RC" "0"
expect_eq "rotate: output" "$OUT" "$CONFIRMED"
assert_routes "rotate" status confirm/start confirm/finish recovery/rotate recovery/confirm
assert_confirm_body "rotate"
for w in "$W3" "$W11" "$W20"; do assert_off_children "rotate" "the word $w" "$w"; done
assert_off_children "rotate" "the vault password" "$VAULT_PW"
assert_off_children "rotate" "the confirm token" "$TOKEN"

# 3. A word left empty is sent as typed (the daemon refuses it), not dropped.
fresh
run_k2 "$W3"$'\n'$'\n'"$W20"$'\n' vault recovery show-once
expect_eq "empty word: exit 0 (the stub accepts it)" "$RC" "0"
if "$REAL_PY" -c '
import json, sys
c = [json.loads(l) for l in open(sys.argv[1]) if l.strip() and json.loads(l)["route"] == "recovery/confirm"]
sys.exit(0 if len(c) == 1 and json.loads(c[0]["body"])["words"] == [sys.argv[2], "", sys.argv[3]] else 1)
' "$REQLOG" "$W3" "$W20"; then ok "empty word: three words sent, the middle one empty"; else bad "empty word: the confirm body is not [w3, \"\", w20]"; fi

echo "vault_recovery_words_off_argv: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
