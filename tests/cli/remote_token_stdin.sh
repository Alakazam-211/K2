#!/usr/bin/env bash
# The remote tokens k2 takes on its command line have a stdin form
# (--token-stdin), and a token given that way never lands in a child
# process's argv or environment, nor k2's own argv (ps shows a command line
# to every account on this computer, and the environment to this user's
# processes). The --token form still works for now and prints one note on
# stderr.
#
# Covered: k2 clone preview --host H --token-stdin.
#
# The real CLI runs against a stub server on TCP that records each request
# (method, path, Authorization header, body). Every python3, curl and env
# the CLI starts is wrapped so its argv and its environment are logged.
#
# No daemon build; a throwaway HOME; never touches the real ~/.k2.
# Runs with the bash 3.2 of macOS as well.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

TOKEN="k2rt_REMOTETOKEN_CHILDTEST"

W="$(mktemp -d "${TMPDIR:-/tmp}/k2rts-XXXXXX")"
ARGLOG="$W/child-argv.log"
ENVLOG="$W/child-env.log"
REQLOG="$W/requests.log"
: >"$ARGLOG"; : >"$ENVLOG"; : >"$REQLOG"

cleanup() {
    if [ -n "${TCP_PID:-}" ]; then
        kill "$TCP_PID" 2>/dev/null || true
        wait "$TCP_PID" 2>/dev/null || true
    fi
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

# A throwaway HOME, outside any K2 session (test process only).
export HOME="$W/home"
mkdir -p "$HOME/.k2"
for p in HOOK_SOCK HOOK_TOKEN CELL PORT HOST REMOTE_TOKEN HOME CONNECT_DB_URL; do
    unset "K5_$p" "K2_$p" "K2SO_$p" 2>/dev/null || true
done

# The stub: this server's daemon and the remote server in one.
"$REAL_PY" - "$W/tcp-port" "$REQLOG" <<'PYEOF' &
import http.server, json, os, sys, threading
port_path, log_path = sys.argv[1], sys.argv[2]
lock = threading.Lock()
class H(http.server.BaseHTTPRequestHandler):
    def _answer(self, status, obj):
        b = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        path = self.path.split("?")[0]
        with lock, open(log_path, "a") as f:
            f.write(json.dumps({"method": self.command, "path": path,
                                "auth": self.headers.get("Authorization", ""), "body": raw}) + "\n")
        if path == "/cli/clone/preview":
            return self._answer(200, {"workspace": "ws1", "secrets": {"rows": []}, "callerCanCopySecrets": True})
        return self._answer(200, {"ok": True})
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(port_path + ".tmp", "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(port_path + ".tmp", port_path)
srv.serve_forever()
PYEOF
TCP_PID=$!
for _ in $(seq 1 50); do [ -s "$W/tcp-port" ] && break; sleep 0.1; done
[ -s "$W/tcp-port" ] || { echo "FAIL: the stub server never came up" >&2; exit 1; }
PORT_NO="$(cat "$W/tcp-port")"
REMOTE="http://127.0.0.1:$PORT_NO"
printf '%s\n' "$PORT_NO" >"$HOME/.k2/heartbeat.port"
printf 'local-test-token\n' >"$HOME/.k2/heartbeat.token"
cd "$W"

PASS=0; FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
expect_eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi
}
fresh() { : >"$ARGLOG"; : >"$ENVLOG"; : >"$REQLOG"; }

run_k2() {
    local input="$1"; shift
    printf 'k2 %s\n' "$*" >>"$ARGLOG"
    RC=0
    OUT="$(printf '%s' "$input" | "$K2_CLI" "$@" 2>"$W/stderr")" || RC=$?
    ERR="$(cat "$W/stderr")"
}

# assert_sent LABEL PATH: a request to PATH carried the token (in its
# Authorization header or its body).
assert_sent() {
    local label="$1" path="$2"
    if "$REAL_PY" -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
sys.exit(0 if any(r["path"] == sys.argv[2] and (sys.argv[3] in r["auth"] or sys.argv[3] in r["body"]) for r in rows) else 1)
' "$REQLOG" "$path" "$TOKEN"; then
        ok "$label: the token reached $path"
    else
        bad "$label: the token never reached $path"
        cut -c1-200 "$REQLOG" >&2
    fi
}

assert_off_children() {
    local label="$1"
    if grep -Fq "$TOKEN" "$ARGLOG"; then
        bad "$label: the token is in a child's argv"
        grep -F "$TOKEN" "$ARGLOG" | cut -c1-200 | head -3 >&2
    else
        ok "$label: the token is in no child's argv"
    fi
    if grep -Fq "$TOKEN" "$ENVLOG"; then
        bad "$label: the token is in a child's environment"
    else
        ok "$label: the token is in no child's environment"
    fi
    if grep -q '^python3 ' "$ARGLOG"; then ok "$label: python3 children were logged"; else bad "$label: no python3 child was logged"; fi
}

NOTE="note: --token is visible to every account on this computer while the command runs; use --token-stdin"

# token_case LABEL PATH ARGS...: the stdin form, then the --token form, of
# one command (ARGS without either), then both together and an empty stdin.
token_case() {
    local label="$1" path="$2"; shift 2
    fresh
    run_k2 "$TOKEN"$'\n' "$@" --token-stdin
    expect_eq "$label --token-stdin: exit 0" "$RC" "0"
    case "$ERR" in *"$NOTE"*) bad "$label --token-stdin: the note was printed" ;; *) ok "$label --token-stdin: no note" ;; esac
    assert_sent "$label --token-stdin" "$path"
    assert_off_children "$label --token-stdin"
    fresh
    run_k2 "" "$@" --token "$TOKEN"
    expect_eq "$label --token: exit 0" "$RC" "0"
    expect_eq "$label --token: the note, once" "$(printf '%s\n' "$ERR" | grep -cF "$NOTE")" "1"
    assert_sent "$label --token" "$path"
    fresh
    run_k2 "$TOKEN"$'\n' "$@" --token-stdin --token "$TOKEN"
    expect_eq "$label both forms: exit 2" "$RC" "2"
    fresh
    run_k2 "" "$@" --token-stdin
    expect_eq "$label empty stdin: exit 2" "$RC" "2"
    if [ -s "$REQLOG" ]; then bad "$label usage errors: a request was sent"; else ok "$label usage errors: nothing sent"; fi
}

# 1. k2 clone preview <ws> --host H.
token_case "clone preview" /cli/clone/preview clone preview ws1 --host "$REMOTE" --json
run_k2 "" clone --help
case "$OUT" in *"--token-stdin                remote token on stdin"*) ok "clone help: --token-stdin is shown" ;; *) bad "clone help: no --token-stdin line" ;; esac

echo "remote_token_stdin: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
