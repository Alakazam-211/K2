#!/usr/bin/env bash
# k2 compute local enable takes the enroll code on stdin (--code-stdin) and a
# one-click ticket on stdin (--ticket-stdin); neither ever lands in a child
# process's argv or environment (ps shows a command line to every account on
# this computer, and the environment to this user's processes).
# --enroll/--code CODE.FP and --ticket T still work for now and print one note
# on stderr.
#
# The real CLI runs against a stub daemon on TCP that answers
# /cli/compute/local/enable and records each request. Every python3, curl
# and env the CLI starts is wrapped so its argv and its environment are
# logged, and k2's own argv is logged too.
#
# No daemon build; a throwaway HOME; never touches the real ~/.k2.
# Runs with the bash 3.2 of macOS as well.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

SECRET_PART="ENROLLCODE-CHILDTEST"
CODE="$SECRET_PART.0123456789abcdef"
TICKET="k2tk1_TICKET-CHILDTEST"

W="$(mktemp -d "${TMPDIR:-/tmp}/k2cle-XXXXXX")"
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

export HOME="$W/home"
mkdir -p "$HOME/.k2"
for p in HOOK_SOCK HOOK_TOKEN CELL PORT HOST REMOTE_TOKEN HOME; do
    unset "K5_$p" "K2_$p" "K2SO_$p" 2>/dev/null || true
done

"$REAL_PY" - "$W/tcp-port" "$REQLOG" <<'PYEOF' &
import http.server, json, os, sys, threading
port_path, log_path = sys.argv[1], sys.argv[2]
lock = threading.Lock()
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode("utf-8", "replace") if n else ""
        path = self.path.split("?")[0]
        with lock, open(log_path, "a") as f:
            f.write(json.dumps({"method": self.command, "path": path, "body": raw}) + "\n")
        if path == "/cli/compute/local/enable":
            status, out = 200, {"message": "Installed and enrolled.", "sas": "123 456"}
        else:
            status, out = 404, {"error": {"code": "not_found", "hint": "the stub answers local enable only"}}
        b = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
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
fresh() { : >"$ARGLOG"; : >"$ENVLOG"; : >"$REQLOG"; }

run_k2() {
    local input="$1"; shift
    printf 'k2 %s\n' "$*" >>"$ARGLOG"
    RC=0
    OUT="$(printf '%s' "$input" | "$K2_CLI" "$@" 2>"$W/stderr")" || RC=$?
    ERR="$(cat "$W/stderr")"
}

# assert_body LABEL JSON: exactly one enable request, with this body.
assert_body() {
    local label="$1" want="$2"
    if "$REAL_PY" -c '
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
if len(rows) != 1 or rows[0]["path"] != "/cli/compute/local/enable":
    sys.exit("requests: %r" % rows)
got = json.loads(rows[0]["body"])
want = json.loads(sys.argv[2])
if got != want:
    sys.exit("body %r, want %r" % (got, want))
' "$REQLOG" "$want" 2>"$W/body-err"; then
        ok "$label: the request body is exactly what was asked"
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

CTRL="https://ctrl.example.test"
ENABLED="Installed and enrolled.
Code: 123 456 — confirm it on the controller: k2 compute node confirm <name> <code>"
WANT_CODE="{\"controller\": \"$CTRL\", \"enroll\": \"$CODE\", \"name\": \"mini-1\", \"privateHome\": true}"

# 1. --code-stdin: the code on stdin only.
fresh
run_k2 "$CODE"$'\n' compute local enable --controller "$CTRL" --code-stdin --name mini-1 --private-home
expect_eq "--code-stdin: exit 0" "$RC" "0"
expect_eq "--code-stdin: output" "$OUT" "$ENABLED"
expect_eq "--code-stdin: no note on stderr" "$ERR" ""
assert_body "--code-stdin" "$WANT_CODE"
if grep -q '^python3 ' "$ARGLOG" && grep -q '^curl ' "$ARGLOG"; then ok "--code-stdin: python3 and curl children were logged"; else bad "--code-stdin: the wrappers saw no python3 or curl child"; fi
assert_off_children "--code-stdin" "the enroll code" "$SECRET_PART"

# 2. --enroll and --code on the command line: still work, one note each.
for flag in --enroll --code; do
    fresh
    run_k2 "" compute local enable --controller "$CTRL" "$flag" "$CODE" --name mini-1 --private-home
    expect_eq "$flag: exit 0" "$RC" "0"
    expect_eq "$flag: output" "$OUT" "$ENABLED"
    expect_eq "$flag: the note" "$ERR" "note: $flag is visible to every account on this computer while the command runs; use --code-stdin"
    assert_body "$flag" "$WANT_CODE"
done

# 3. Both forms, or an empty stdin: usage (exit 2), nothing sent.
fresh
run_k2 "$CODE"$'\n' compute local enable --controller "$CTRL" --code-stdin --enroll "$CODE" --name mini-1
expect_eq "both forms: exit 2" "$RC" "2"
expect_eq "both forms: the error" "$ERR" "k2 compute local enable: --code-stdin or --enroll, not both"
fresh
run_k2 "" compute local enable --controller "$CTRL" --code-stdin --name mini-1
expect_eq "empty stdin: exit 2" "$RC" "2"
expect_eq "empty stdin: the error" "$ERR" "k2 compute local enable: --code-stdin: no code on stdin"
if [ -s "$REQLOG" ]; then bad "usage errors: a request was sent"; else ok "usage errors: nothing sent"; fi

# 4. --ticket-stdin: the ticket on stdin only; --ticket still works with a
#    note; the two together, or with --code-stdin, are usage errors.
WANT_TICKET="{\"controller\": \"$CTRL\", \"ticket\": \"$TICKET\", \"name\": \"mini-1\", \"privateHome\": true}"
fresh
run_k2 "$TICKET"$'\n' compute local enable --controller "$CTRL" --ticket-stdin --name mini-1 --private-home
expect_eq "--ticket-stdin: exit 0" "$RC" "0"
expect_eq "--ticket-stdin: output" "$OUT" "$ENABLED"
expect_eq "--ticket-stdin: no note on stderr" "$ERR" ""
assert_body "--ticket-stdin" "$WANT_TICKET"
assert_off_children "--ticket-stdin" "the ticket" "$TICKET"
fresh
run_k2 "" compute local enable --controller "$CTRL" --ticket "$TICKET" --name mini-1 --private-home
expect_eq "--ticket: exit 0" "$RC" "0"
expect_eq "--ticket: the note" "$ERR" "note: --ticket is visible to every account on this computer while the command runs; use --ticket-stdin"
assert_body "--ticket" "$WANT_TICKET"
fresh
run_k2 "$TICKET"$'\n' compute local enable --controller "$CTRL" --ticket-stdin --ticket "$TICKET" --name mini-1
expect_eq "--ticket-stdin with --ticket: exit 2" "$RC" "2"
expect_eq "--ticket-stdin with --ticket: the error" "$ERR" "k2 compute local enable: --ticket-stdin or --ticket, not both"
run_k2 "$TICKET"$'\n' compute local enable --controller "$CTRL" --ticket-stdin --code-stdin --name mini-1
expect_eq "--ticket-stdin with --code-stdin: exit 2" "$RC" "2"
expect_eq "--ticket-stdin with --code-stdin: the error" "$ERR" "k2 compute local enable: --code-stdin or --ticket-stdin, not both (stdin holds one)"
run_k2 "" compute local enable --controller "$CTRL" --ticket-stdin --name mini-1
expect_eq "--ticket-stdin, empty stdin: exit 2" "$RC" "2"
expect_eq "--ticket-stdin, empty stdin: the error" "$ERR" "k2 compute local enable: --ticket-stdin: no ticket on stdin"
if [ -s "$REQLOG" ]; then bad "ticket usage errors: a request was sent"; else ok "ticket usage errors: nothing sent"; fi

# 5. The help shows --code-stdin first.
fresh
run_k2 "" compute --help
line="$(printf '%s\n' "$OUT" | grep -m1 'k2 compute local enable' || true)"
expect_eq "help: the enable line" "$line" "  k2 compute local enable [--controller URL --code-stdin --name NAME]"

echo "compute_local_enable_stdin: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
