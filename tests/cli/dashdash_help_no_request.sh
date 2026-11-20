#!/usr/bin/env bash
# The words after a bare `--` belong to the command being run, never to k2.
# A -h or --help there is the child's flag: k2 does what it was asked and
# hands it on. A -h or --help BEFORE the `--` is still k2's help: it prints
# the help, exits 0 and sends nothing.
#
# Regression (0.46.17): every help scan read the whole line, so
# `k2 compute run -- ls -h` printed the compute help and never submitted,
# and with no session passport (K2_NO_OWNER_FALLBACK=1)
# `k2 vault run --secret A -- grep --help` counted as a help page and got
# past the no-owner refusal and the connection check.
#
# What it checks, against a stub daemon on a free loopback port that
# records every request (any method, any path):
#   * compute run -- ls -h / --help: the stub records the run POST with
#     argv [ls, -h] / [ls, --help];
#   * vault run --secret A -- grep --help: the CLI asks for the run helper
#     and execs it with `-- grep --help`;
#   * -h/--help before `--` on compute run and vault run: the help, exit 0,
#     nothing sent, no helper run;
#   * with no passport (K2_NO_OWNER_FALLBACK=1, no token): a run whose only
#     -h/--help is after `--` is refused with exit 3 and sends nothing,
#     while `vault run -h ...` still prints its help.
# HOME is a temp dir, no real daemon, no network. Loud: every failure is
# named and the script exits 1.
# Run with: bash tests/cli/dashdash_help_no_request.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
for v in $(env | grep -E '^K(2|2SO|5)_' | cut -d= -f1); do
    case "$v" in K2_TEST_HOME) ;; *) unset "$v" ;; esac
done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-dashdash-help-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
    hermetic_cli_cleanup
}
trap cleanup EXIT

REQ_LOG="$WORK/requests.log"
HELPER="$WORK/fake-helper"
HELPER_LOG="$WORK/helper.log"
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"

# The fake run helper writes its argv, one word per line, and exits 0.
cat >"$HELPER" <<'EOF'
#!/usr/bin/env bash
for a in "$@"; do printf 'ARG=%s\n' "$a"; done >"$FAKE_HELPER_LOG"
EOF
chmod +x "$HELPER"

python3 - "$WORK" "$HELPER" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work, helper = sys.argv[1], sys.argv[2]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _reply(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n) if n else b""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s\n" % (self.command, path))
        if path == "/cli/compute/run":
            with open(work + "/run.json", "wb") as f:
                f.write(body)
            # A refusal ends the run at once; the body is what is checked.
            self._reply(400, {"error": {"code": "stub", "hint": "stub daemon"}})
        elif path == "/cli/vault/run-helper":
            self._reply(200, {"helper": helper, "tokenFd": True})
        else:
            self._reply(404, {"error": {"code": "not_found", "hint": "no route"}})
    def do_GET(self):
        self._handle()
    def do_POST(self):
        self._handle()
srv = HTTPServer(("127.0.0.1", 0), H)
with open(work + "/port", "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
for _ in $(seq 1 50); do
    [ -s "$WORK/port" ] && break
    sleep 0.1
done
[ -s "$WORK/port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/port")"
cd "$WORK/proj"

# run <args...>: the CLI as a session (port and passport), from a folder
# that is not a git repo. Sets OUT, ERR, RC. stdin is empty.
run() {
    set +e
    OUT="$(env K2_PORT="$STUB_PORT" K2_HOOK_TOKEN=passport-for-test \
        K2_PROJECT_PATH="$WORK/proj" FAKE_HELPER_LOG="$HELPER_LOG" \
        "$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

# run_no_owner <args...>: no passport and K2_NO_OWNER_FALLBACK=1 (a K2
# virtual desktop, or a command under k2 vault run).
run_no_owner() {
    set +e
    OUT="$(env K2_PORT="$STUB_PORT" K2_NO_OWNER_FALLBACK=1 \
        K2_PROJECT_PATH="$WORK/proj" FAKE_HELPER_LOG="$HELPER_LOG" \
        "$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

reset() { : >"$REQ_LOG"; rm -f "$WORK/run.json" "$HELPER_LOG"; }

sent_nothing() {
    local label="$1"
    if [ -s "$REQ_LOG" ]; then
        bad "$label: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$label: sent nothing"
    fi
    if [ -e "$HELPER_LOG" ]; then
        bad "$label: ran the helper:"; sed 's/^/      | /' "$HELPER_LOG" >&2
    else
        ok "$label: ran nothing"
    fi
}

# help_is <ref> <args...>: prints exactly <ref> on stdout, nothing on
# stderr, exits 0 and sends nothing.
help_is() {
    local want="$1"; shift
    local label="k2 $*"
    reset
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$want" ] && ok "$label: prints the help" \
        || { bad "$label: stdout is not the help"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
}

# run_argv_is <json argv> <args...>: the run POST went out with that argv.
run_argv_is() {
    local want="$1"; shift
    local label="k2 $*"
    reset
    run "$@"
    if [ ! -s "$WORK/run.json" ]; then
        bad "$label: sent no run (exit $RC)"; printf '%s\n' "$OUT" "$ERR" | sed 's/^/      | /' >&2
        return
    fi
    local got
    got="$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["argv"]))' "$WORK/run.json")"
    [ "$got" = "$want" ] && ok "$label: the run POST carries argv $want" \
        || bad "$label: argv $got (want $want)"
}

echo "== compute run: -h/--help after -- is the child's =="
run_argv_is '["ls", "-h"]' compute run --any -- ls -h
run_argv_is '["ls", "--help"]' compute run --any -- ls --help
run_argv_is '["sh", "-c", "x", "-h", "--", "--help"]' compute run --any -- sh -c x -h -- --help

echo "== compute run: -h/--help before -- is k2's help =="
reset
run compute run --help
COMPUTE_REF="$OUT"
[ "$RC" -eq 0 ] && [ -n "$COMPUTE_REF" ] && ok "k2 compute run --help: exit 0, prints help" \
    || bad "k2 compute run --help: exit $RC, stdout '$COMPUTE_REF'"
sent_nothing "k2 compute run --help"
help_is "$COMPUTE_REF" compute run --any -h -- ls
help_is "$COMPUTE_REF" compute run -h -- ls -h
help_is "$COMPUTE_REF" compute run --any --help -- ls --help

echo "== vault run: -h/--help after -- is the child's =="
reset
run vault run --secret A -- grep --help
[ "$RC" -eq 0 ] && ok "k2 vault run --secret A -- grep --help: exit 0" \
    || { bad "k2 vault run --secret A -- grep --help: exit $RC"; printf '%s\n' "$OUT" "$ERR" | sed 's/^/      | /' >&2; }
[ "$(cat "$REQ_LOG")" = "GET /cli/vault/run-helper" ] && ok "vault run asks only for the run helper" \
    || { bad "vault run requests:"; sed 's/^/      | /' "$REQ_LOG" >&2; }
if [ -s "$HELPER_LOG" ]; then
    got="$(python3 - "$HELPER_LOG" <<'PY'
import json, sys
args = [l[4:] for l in open(sys.argv[1]).read().splitlines() if l.startswith("ARG=")]
print(json.dumps(args[args.index("--"):]))
PY
)"
    [ "$got" = '["--", "grep", "--help"]' ] && ok "the helper runs grep --help" \
        || bad "the helper's command: $got (want [\"--\", \"grep\", \"--help\"])"
else
    bad "vault run ran no helper"
fi

echo "== vault run: -h/--help before -- is k2's help =="
reset
run vault run --help
VAULT_REF="$OUT"
[ "$RC" -eq 0 ] && [ -n "$VAULT_REF" ] && ok "k2 vault run --help: exit 0, prints help" \
    || bad "k2 vault run --help: exit $RC, stdout '$VAULT_REF'"
sent_nothing "k2 vault run --help"
help_is "$VAULT_REF" vault run -h --secret A -- grep
help_is "$VAULT_REF" vault run --secret A --help -- grep -h

echo "== no passport: a -h/--help after -- is not a help page =="
for args in "vault run --secret A -- grep --help" "compute run --any -- ls -h"; do
    reset
    # shellcheck disable=SC2086 # word-splitting the case is the point
    run_no_owner $args
    [ "$RC" -eq 3 ] && ok "no passport, k2 $args: exit 3" \
        || { bad "no passport, k2 $args: exit $RC (want 3)"; printf '%s\n' "$OUT" "$ERR" | sed 's/^/      | /' >&2; }
    case "$ERR" in
        *"won't act as this server's owner"*) ok "no passport, k2 $args: says why" ;;
        *) bad "no passport, k2 $args: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2 ;;
    esac
    sent_nothing "no passport, k2 $args"
done
reset
run_no_owner vault run -h --secret A -- grep
[ "$RC" -eq 0 ] && [ "$OUT" = "$VAULT_REF" ] && ok "no passport, k2 vault run -h ...: the help, exit 0" \
    || { bad "no passport, k2 vault run -h ...: exit $RC"; printf '%s\n' "$OUT" "$ERR" | sed 's/^/      | /' >&2; }
sent_nothing "no passport, k2 vault run -h ..."

echo ""
echo "dashdash_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
