#!/usr/bin/env bash
# k2 ci signer add takes the pairing code on stdin (--code-stdin), and the
# code never lands in a child process's argv or environment (ps shows a
# command line to every account on this computer, and the environment to this
# user's processes). The <CODE.FP> word still works for now and prints one
# note on stderr that other users can see it.
#
# The real CLI runs against two stubs: one on the vault socket that answers
# the vault password confirm with a token, and one on the daemon's TCP port
# that answers /cli/ci/signer/add and records each request. Every python3,
# curl and env the CLI starts is wrapped so its argv and its environment are
# logged, and k2's own argv is logged too.
#
# No daemon build; a throwaway HOME; never touches the real ~/.k2.
# Runs with the bash 3.2 of macOS as well.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

SECRET_PART="SIGNCODE-CHILDTEST"
CODE="$SECRET_PART.f00dfeed01"
VAULT_PW="SIGNVAULTPW_CHILDTEST"
TOKEN="k2vc_SIGNCHILDTEST_token"
NOTE="note: <CODE.FP> is visible to every user on this computer while the command runs; use --code-stdin"

# /tmp, not $TMPDIR: a unix socket path must stay short on macOS.
W="$(mktemp -d /tmp/k2sig-XXXXXX)"
ARGLOG="$W/child-argv.log"
ENVLOG="$W/child-env.log"
REQLOG="$W/requests.log"
: >"$ARGLOG"; : >"$ENVLOG"; : >"$REQLOG"

cleanup() {
    local p
    for p in ${VAULT_PID:-} ${TCP_PID:-}; do
        kill "$p" 2>/dev/null || true
        wait "$p" 2>/dev/null || true
    done
    rm -rf "$W"
}
trap cleanup EXIT

REAL_PY="$(command -v python3)" || { echo "FAIL: python3 not found" >&2; exit 1; }
REAL_CURL="$(command -v curl)" || { echo "FAIL: curl not found" >&2; exit 1; }
REAL_ENV="$(command -v env)" || { echo "FAIL: env not found" >&2; exit 1; }

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

# A throwaway HOME. Nothing may mark this a K2 session (test process only):
# signer pairing is a person's.
export HOME="$W/home"
mkdir -p "$HOME/.k2"
for p in HOOK_SOCK HOOK_TOKEN CELL PORT HOST REMOTE_TOKEN HOME VAULT_CONFIRM_METHOD; do
    unset "K5_$p" "K2_$p" "K2SO_$p" 2>/dev/null || true
done

# Both stubs in one python: the vault socket and the daemon's TCP port.
"$REAL_PY" - "$HOME/.k2/vault.sock" "$W" "$REQLOG" "$TOKEN" <<'PYEOF' &
import http.server, json, os, socket, sys, threading
sock_path, work, log_path, token = sys.argv[1:5]
lock = threading.Lock()
def log(kind, route, body):
    with lock, open(log_path, "a") as f:
        f.write(json.dumps({"via": kind, "route": route, "body": body}) + "\n")

srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(sock_path)
srv.listen(16)
def vault_handle(conn):
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = conn.recv(65536)
        if not chunk:
            break
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
    parts = head.split(b"\r\n", 1)[0].decode("latin1").split(" ")
    route = parts[1].split("?")[0].rsplit("/cli/vault/", 1)[-1] if len(parts) > 1 else ""
    log("vault", route, rest.decode("utf-8", "replace"))
    if route == "confirm/start":
        out = {"methods": ["vault-password"], "expiresIn": 60}
    elif route == "confirm/finish":
        out = {"token": token, "expiresIn": 60}
    else:
        out = {"ok": True}
    payload = json.dumps(out).encode()
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s" % (len(payload), payload))
    conn.close()
def vault_loop():
    while True:
        c, _ = srv.accept()
        threading.Thread(target=vault_handle, args=(c,), daemon=True).start()
threading.Thread(target=vault_loop, daemon=True).start()

class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        path = self.path.split("?")[0]
        log("tcp", path, raw)
        if path == "/cli/ci/signer/add":
            status, out = 200, {"pairing": "csp_1", "offer": {"v": 1, "pairing": "csp_1"}}
        else:
            status, out = 404, {"error": {"code": "not_found", "hint": "the stub answers signer add only"}}
        b = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
tcp = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "tcp-port.tmp"), "w") as f:
    f.write(str(tcp.server_address[1]))
os.rename(os.path.join(work, "tcp-port.tmp"), os.path.join(work, "tcp-port"))
tcp.serve_forever()
PYEOF
TCP_PID=$!
for _ in $(seq 1 50); do [ -s "$W/tcp-port" ] && [ -S "$HOME/.k2/vault.sock" ] && break; sleep 0.1; done
[ -s "$W/tcp-port" ] && [ -S "$HOME/.k2/vault.sock" ] || { echo "FAIL: the stubs never came up" >&2; exit 1; }
cat "$W/tcp-port" >"$HOME/.k2/heartbeat.port"
printf 'test-token\n' >"$HOME/.k2/heartbeat.token"
printf '{"hello": {"v": 1, "signer": "s1"}}\n' >"$W/pair.json"
cd "$W"

PASS=0; FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
expect_eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi
}
fresh() { : >"$ARGLOG"; : >"$ENVLOG"; : >"$REQLOG"; rm -f "$W/offer.json"; }

# run_k2 STDIN ARGS...: sets OUT, ERR and RC; k2's own argv is logged.
run_k2() {
    local input="$1"; shift
    printf 'k2 %s\n' "$*" >>"$ARGLOG"
    RC=0
    OUT="$(printf '%s' "$input" | "$K2_CLI" "$@" 2>"$W/stderr")" || RC=$?
    ERR="$(cat "$W/stderr")"
}

# The daemon got exactly one signer add whose body is the code, the hello
# and the confirm token.
assert_add_body() {
    local label="$1"
    if "$REAL_PY" -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
adds = [r for r in rows if r["via"] == "tcp"]
if len(adds) != 1 or adds[0]["route"] != "/cli/ci/signer/add":
    sys.exit("daemon requests: %r" % adds)
got = json.loads(adds[0]["body"])
want = {"code": sys.argv[2], "hello": {"v": 1, "signer": "s1"}, "confirmToken": sys.argv[3]}
if got != want:
    sys.exit("body %r, want %r" % (got, want))
' "$REQLOG" "$CODE" "$TOKEN" 2>"$W/body-err"; then
        ok "$label: the signer add body is the code, the hello and the token"
    else
        bad "$label: $(cat "$W/body-err")"
    fi
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

OFFER_LINES="pairing csp_1: offer written to offer.json
on the signing machine: sudo k2-signer-admin pair-reveal < offer.json > reveal.json
then here: k2 ci signer reveal csp_1 --reveal reveal.json"

# 1. --code-stdin: the code, then the vault password, on stdin.
fresh
run_k2 "$CODE"$'\n'"$VAULT_PW"$'\n' ci signer add --code-stdin --hello pair.json --out offer.json
expect_eq "--code-stdin: exit 0" "$RC" "0"
expect_eq "--code-stdin: output" "$OUT" "$OFFER_LINES"
case "$ERR" in *"$NOTE"*) bad "--code-stdin: the command-line note was printed" ;; *) ok "--code-stdin: no command-line note" ;; esac
assert_add_body "--code-stdin"
expect_eq "--code-stdin: the offer file" "$(cat "$W/offer.json" 2>/dev/null)" '{"v": 1, "pairing": "csp_1"}'
if grep -q '^python3 ' "$ARGLOG" && grep -q '^curl ' "$ARGLOG"; then ok "--code-stdin: python3 and curl children were logged"; else bad "--code-stdin: the wrappers saw no python3 or curl child"; fi
assert_off_children "--code-stdin" "the pairing code" "$SECRET_PART"
assert_off_children "--code-stdin" "the vault password" "$VAULT_PW"
assert_off_children "--code-stdin" "the confirm token" "$TOKEN"

# 2. <CODE.FP> on the command line: still works, with one note on stderr.
fresh
run_k2 "$VAULT_PW"$'\n' ci signer add "$CODE" --hello pair.json --out offer.json
expect_eq "<CODE.FP>: exit 0" "$RC" "0"
expect_eq "<CODE.FP>: output" "$OUT" "$OFFER_LINES"
expect_eq "<CODE.FP>: the note, once" "$(printf '%s\n' "$ERR" | grep -cF "$NOTE")" "1"
assert_add_body "<CODE.FP>"

# 3. Both forms, an empty stdin, no hello: usage (exit 2), nothing sent.
fresh
run_k2 "$CODE"$'\n' ci signer add "$CODE" --code-stdin --hello pair.json
expect_eq "both forms: exit 2" "$RC" "2"
expect_eq "both forms: the error" "$ERR" "k2 ci signer add: --code-stdin or <CODE.FP>, not both"
fresh
run_k2 "" ci signer add --code-stdin --hello pair.json
expect_eq "empty stdin: exit 2" "$RC" "2"
expect_eq "empty stdin: the error" "$ERR" "k2 ci signer add: --code-stdin: no code on stdin"
fresh
run_k2 "$CODE"$'\n' ci signer add --code-stdin
expect_eq "no --hello: exit 2" "$RC" "2"
case "$ERR" in "usage: k2 ci signer add --code-stdin --hello"*) ok "no --hello: the usage names --code-stdin first" ;; *) bad "no --hello: usage $(printf %q "$ERR")" ;; esac
if [ -s "$REQLOG" ]; then bad "usage errors: a request was sent"; else ok "usage errors: nothing sent"; fi

# 4. The help shows --code-stdin first.
fresh
run_k2 "" ci --help
first="$(printf '%s\n' "$OUT" | grep -m1 'k2 ci signer add' || true)"
expect_eq "help: the signer add line" "$first" "  k2 ci signer add --code-stdin --hello pair.json [--out offer.json] < code"

echo "ci_signer_code_stdin: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
