#!/usr/bin/env bash
# Privacy check: a test guest's password, typed on stdin, never lands in a
# child process's argv or environment (ps shows a command line to every
# account on this computer, and the environment to this user's processes). It
# reaches the request on a pipe only.
#
#   k2 app user add <app> <name> --test [--room R] --password-stdin
#   k2 app user password <name> --test --password-stdin
#
# The real CLI runs against a stub daemon on TCP that records each request.
# Every python3, curl and env the CLI starts is wrapped so its argv and its
# environment are logged, and k2's own argv is logged too. The password
# must reach the stub's request body and appear in no logged argv and no
# logged environment. The commands' output and exit codes are checked as
# well, so the feature still works: the add without --password-stdin (K2
# makes the password), --json, and an empty stdin (exit 2, nothing sent).
#
# No daemon build; a throwaway HOME; never touches the real ~/.k2.
# Runs with the bash 3.2 of macOS as well.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

PASS_SENTINEL="TGPW_CHILDTEST_9xQ2"

W="$(mktemp -d "${TMPDIR:-/tmp}/k2tgpw-XXXXXX")"
ARGLOG="$W/child-argv.log"
ENVLOG="$W/child-env.log"
TCPLOG="$W/tcp-requests.log"
: >"$ARGLOG"; : >"$ENVLOG"; : >"$TCPLOG"

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

# Wrappers that record their argv and their environment, then run the real
# program unchanged. Their shebang names /usr/bin/env by path, so the env
# wrapper never runs itself.
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

# A throwaway HOME. Nothing may mark this a K2 session (test process only).
export HOME="$W/home"
mkdir -p "$HOME/.k2"
for p in HOOK_SOCK HOOK_TOKEN CELL PORT HOST REMOTE_TOKEN; do
    unset "K5_$p" "K2_$p" "K2SO_$p" 2>/dev/null || true
done
# The expiry line prints in local time.
export TZ=UTC

# The daemon stub: logs every request as one JSON line and answers the two
# test-guest routes the way the daemon does.
"$REAL_PY" - "$W/tcp-port" "$TCPLOG" <<'PYEOF' &
import http.server, json, os, sys, threading
port_path, log_path = sys.argv[1], sys.argv[2]
lock = threading.Lock()
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
            f.write(json.dumps({"method": self.command, "path": path, "body": raw}) + "\n")
        body = json.loads(raw) if raw.strip() else {}
        if path == "/cli/app/test-guest/add":
            out = {"username": body.get("username"), "app": body.get("app"), "role": body.get("role"),
                   "rooms": body.get("rooms") or [], "expiresAt": 2000000000, "tied": True, "note": "a note"}
            if not body.get("password"):
                out["password"] = "k2-made-pw-1"
            return self._answer(200, out)
        if path == "/cli/app/test-guest/password":
            return self._answer(200, {"username": body.get("username"), "once": True})
        return self._answer(404, {"error": {"code": "not_found", "hint": "the stub answers the test-guest routes only"}})
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
[ -s "$W/tcp-port" ] || { echo "FAIL: the stub daemon never came up" >&2; exit 1; }
cat "$W/tcp-port" >"$HOME/.k2/heartbeat.port"
printf 'test-token\n' >"$HOME/.k2/heartbeat.token"
cd "$W"

PASS=0; FAIL=0
ok() { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }
expect_eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $(printf %q "$2") want $(printf %q "$3"))"; fi
}

fresh() { : >"$ARGLOG"; : >"$ENVLOG"; : >"$TCPLOG"; }

# run_k2 STDIN ARGS...: runs k2 with STDIN on its stdin; sets OUT, ERR, RC.
# k2's own argv is logged with the children's.
run_k2() {
    local input="$1"; shift
    printf 'k2 %s\n' "$*" >>"$ARGLOG"
    RC=0
    OUT="$(printf '%s' "$input" | "$K2_CLI" "$@" 2>"$W/stderr")" || RC=$?
    ERR="$(cat "$W/stderr")"
}

# The wrappers really ran (else every "not in argv" below passes vacuously).
assert_children_seen() {
    local label="$1" prog
    for prog in python3 curl; do
        if grep -q "^$prog " "$ARGLOG"; then ok "$label: $prog children were logged"; else bad "$label: no $prog child was logged"; fi
    done
}

# The password is in no logged argv (k2's own included) and no logged
# environment.
assert_off_children() {
    local label="$1"
    if grep -Fq "$PASS_SENTINEL" "$ARGLOG"; then
        bad "$label: the password is in a child's argv"
        grep -F "$PASS_SENTINEL" "$ARGLOG" | head -3 >&2
    else
        ok "$label: no password in any child's argv"
    fi
    if grep -Fq "$PASS_SENTINEL" "$ENVLOG"; then
        bad "$label: the password is in a child's environment"
        grep -F "$PASS_SENTINEL" "$ENVLOG" | head -3 >&2
    else
        ok "$label: no password in any child's environment"
    fi
}

# assert_body LABEL PATH JSON: the stub got exactly one request, a POST to
# PATH whose body parses to JSON (key order aside).
assert_body() {
    local label="$1" path="$2" want="$3"
    if "$REAL_PY" -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
want = json.loads(sys.argv[3])
if len(rows) != 1:
    sys.exit("%d requests: %r" % (len(rows), rows))
r = rows[0]
if (r["method"], r["path"]) != ("POST", sys.argv[2]):
    sys.exit("request %s %s" % (r["method"], r["path"]))
got = json.loads(r["body"])
if got != want:
    sys.exit("body %r, want %r" % (got, want))
' "$TCPLOG" "$path" "$want" 2>"$W/body-err"; then
        ok "$label: the request body is exactly what was asked"
    else
        bad "$label: $(cat "$W/body-err")"
    fi
}

ADD_LINES="Test guest qa-tg of App acme (role none, rooms r1), ends 2033-05-18 03:33
tied to your workspace: it never counts as a person for person-only cards
a note"

# 1. add --password-stdin, with a room (rooms build in their own python3).
fresh
run_k2 "$PASS_SENTINEL"$'\n' app user add acme qa-tg --test --room r1 --password-stdin
expect_eq "add --password-stdin: exit 0" "$RC" "0"
expect_eq "add --password-stdin: output" "$OUT" "$ADD_LINES"
assert_body "add --password-stdin" /cli/app/test-guest/add \
    "{\"app\": \"acme\", \"username\": \"qa-tg\", \"rooms\": [\"r1\"], \"password\": \"$PASS_SENTINEL\"}"
assert_children_seen "add --password-stdin"
assert_off_children "add --password-stdin"

# 2. add --password-stdin --json: the daemon's answer as it came.
fresh
run_k2 "$PASS_SENTINEL"$'\n' app user add acme qa-tg --test --role tester --expires 2h --password-stdin --json
expect_eq "add --json: exit 0" "$RC" "0"
expect_eq "add --json: the answer as sent" "$("$REAL_PY" -c 'import json,sys; v=json.loads(sys.argv[1]); print(v["username"], v["role"], "password" in v)' "$OUT")" "qa-tg tester False"
assert_body "add --json" /cli/app/test-guest/add \
    "{\"app\": \"acme\", \"username\": \"qa-tg\", \"role\": \"tester\", \"expires\": \"2h\", \"password\": \"$PASS_SENTINEL\"}"
assert_off_children "add --json"

# 3. password --test --password-stdin.
fresh
run_k2 "$PASS_SENTINEL"$'\n' app user password qa-tg --test --password-stdin
expect_eq "password --password-stdin: exit 0" "$RC" "0"
expect_eq "password --password-stdin: output" "$OUT" "Test guest qa-tg: password set (the one set after the owner approved it)."
assert_body "password --password-stdin" /cli/app/test-guest/password \
    "{\"username\": \"qa-tg\", \"password\": \"$PASS_SENTINEL\"}"
assert_children_seen "password --password-stdin"
assert_off_children "password --password-stdin"

# 4. add without --password-stdin: no password is sent, K2 makes one and
#    it is shown once.
fresh
run_k2 "" app user add acme qa-tg --test --room r1
expect_eq "add (K2 makes the password): exit 0" "$RC" "0"
expect_eq "add (K2 makes the password): output" "$OUT" "Test guest qa-tg of App acme (role none, rooms r1), ends 2033-05-18 03:33
password (shown once): k2-made-pw-1
tied to your workspace: it never counts as a person for person-only cards
a note"
assert_body "add (K2 makes the password)" /cli/app/test-guest/add \
    "{\"app\": \"acme\", \"username\": \"qa-tg\", \"rooms\": [\"r1\"]}"

# 5. An empty stdin is a usage error (exit 2) and sends nothing.
for verb in add password; do
    fresh
    if [ "$verb" = add ]; then
        run_k2 "" app user add acme qa-tg --test --password-stdin
    else
        run_k2 "" app user password qa-tg --test --password-stdin
    fi
    expect_eq "$verb, empty stdin: exit 2" "$RC" "2"
    expect_eq "$verb, empty stdin: the error" "$ERR" "--password-stdin: no password on stdin"
    if [ -s "$TCPLOG" ]; then bad "$verb, empty stdin: a request was sent"; else ok "$verb, empty stdin: nothing sent"; fi
done

echo "app_test_guest_password_off_argv: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
