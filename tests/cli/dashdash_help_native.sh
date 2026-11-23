#!/usr/bin/env bash
# A -h after a bare `--` is not k2's help, and the binary answers such a
# line itself, as the shell does.
#
# The shell's help checks (`_wants_help "$@"`) stop at the first bare
# `--`: what follows is the command's (a name, a message, a child
# command). The binary's families (organization, sessions, wiki,
# project, focus-group, zen, vault, terminal) read the whole line for
# -h, so they handed every such line to the shell. Now their scans stop
# at `--` too, and the binary answers the line natively.
#
# Each row runs the line on bash (cli/k2) and checks bash's own answer
# (not the help: the request it sends, or its usage line). When the CLI
# under test is the binary, the row runs it too, with its hand-over to
# the shell broken (the K2 home's run/cli is a file, so the binary can't
# write the shell CLI there), and its exit, stdout, stderr, requests and
# helper command must be bash's, byte for byte. A line the binary hands
# on fails there, naming run/cli: the first binary row proves that.
#
# HOME is a temp dir, a stub daemon on a free loopback port records
# every request, no network. Loud: every failure is named, exit 1.
# Run with: bash tests/cli/dashdash_help_native.sh
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

WORK="$(mktemp -d -t k2-dashdash-native-XXXXXX)"
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

HELPER="$WORK/fake-helper"
mkdir -p "$WORK/proj/.k2"
# The Garden hint prints once per workspace: marked as shown, so neither
# side prints it.
printf '0.46.3\n' >"$WORK/proj/.k2/.garden-changes-hint"
# The binary's hand-over writes the shell CLI to <K2 home>/run/cli: a file
# there makes every hand-over fail. bash never reads it.
mkdir -p "$HOME/.k2/run"
: >"$HOME/.k2/run/cli"

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
        # Every request: method, path with its query, and body.
        with open(work + "/requests", "ab") as f:
            f.write(("%s %s " % (self.command, urllib.parse.unquote_plus(self.path))).encode())
            f.write(urllib.parse.unquote_plus(body.decode("utf-8", "replace")).encode() + b"\n")
        if path == "/cli/vault/run-helper":
            self._reply(200, {"helper": helper, "tokenFd": True})
        else:
            self._reply(200, {"ok": True})
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

# side <name> <cli> <args...>: one run as a session (port and passport),
# stdin closed, stopped after 30 s (exit 124). Leaves <name>.out, .err,
# .code, .req (the requests) and .helper (the helper's words from its
# `--` on) in $WORK.
side() {
    local name="$1" cli="$2"; shift 2
    : >"$WORK/requests"
    rm -f "$WORK/helper.log"
    set +e
    env K2_PORT="$STUB_PORT" K2_HOOK_TOKEN=passport-for-test \
        K2_PROJECT_PATH="$WORK/proj" FAKE_HELPER_LOG="$WORK/helper.log" \
        python3 -c '
import subprocess, sys
out, err = sys.argv[1], sys.argv[2]
try:
    with open(out, "wb") as o, open(err, "wb") as e:
        p = subprocess.run(sys.argv[3:], stdin=subprocess.DEVNULL, stdout=o, stderr=e, timeout=30)
    sys.exit(p.returncode)
except subprocess.TimeoutExpired:
    sys.exit(124)
' "$WORK/$name.out" "$WORK/$name.err" "$cli" "$@"
    echo "$?" >"$WORK/$name.code"
    set -e
    cp "$WORK/requests" "$WORK/$name.req"
    if [ -e "$WORK/helper.log" ]; then
        python3 - "$WORK/helper.log" >"$WORK/$name.helper" <<'PY'
import json, sys
args = [l[4:] for l in open(sys.argv[1]).read().splitlines() if l.startswith("ARG=")]
print(json.dumps(args[args.index("--"):] if "--" in args else ["no --"] + args))
PY
    else
        : >"$WORK/$name.helper"
    fi
}

show() { sed 's/^/      | /' "$1" >&2; }

# The binary answers the line itself: the same exit, stdout, stderr,
# requests and helper command as bash, and no hand-over.
same_as_bash() {
    local label="$1" f
    if grep -q "run/cli" "$WORK/bin.err"; then
        bad "$label: the binary handed it to the shell"; show "$WORK/bin.err"
        return
    fi
    for f in code out err req helper; do
        if cmp -s "$WORK/bash.$f" "$WORK/bin.$f"; then
            ok "$label: the binary's $f is bash's"
        else
            bad "$label: the binary's $f differs from bash's"
            diff "$WORK/bash.$f" "$WORK/bin.$f" | head -8 >&2 || true
        fi
    done
}

# row <check> <want1> <want2> -- <args...>: the line on bash, then on the
# binary. Checks of bash's own answer:
#   req  <a> <b>  a request line holds both <a> and <b>;
#   stop <code> <text>  exit <code>, <text> on stderr, nothing sent.
# Then, on the binary, the help after `--` is still never the help, and
# the answer is bash's.
row() {
    local check="$1" a="$2" b="$3"; shift 3
    [ "$1" = "--" ] && shift
    local label="k2 $*"
    side bash "$CLI_BASH_SOURCE" "$@"
    case "$check" in
        req)
            if grep -F -- "$a" "$WORK/bash.req" | grep -qF -- "$b"; then
                ok "$label: bash sends $a with $b"
            else
                bad "$label: bash sent no $a with $b (exit $(cat "$WORK/bash.code"))"
                show "$WORK/bash.req"; show "$WORK/bash.err"
            fi
            ;;
        stop)
            if [ "$(cat "$WORK/bash.code")" = "$a" ] && grep -qF -- "$b" "$WORK/bash.err" \
                && [ ! -s "$WORK/bash.req" ]; then
                ok "$label: bash says $b, exit $a, nothing sent"
            else
                bad "$label: bash exit $(cat "$WORK/bash.code") (want $a and '$b', nothing sent)"
                show "$WORK/bash.err"; show "$WORK/bash.req"
            fi
            ;;
        *) echo "FAIL: no check '$check'" >&2; exit 1 ;;
    esac
    if [ "$K2" != "$CLI_BASH_SOURCE" ]; then
        side bin "$K2" "$@"
        same_as_bash "$label"
    fi
}

if [ "$K2" != "$CLI_BASH_SOURCE" ]; then
    echo "== the hand-over is broken: a line the binary hands on fails =="
    # Every app line with a bare `--` (but exec and shell) is the shell's.
    side bin "$K2" app logs -- acme
    if [ "$(cat "$WORK/bin.code")" != 0 ] && grep -q "run/cli" "$WORK/bin.err" \
        && [ ! -s "$WORK/bin.req" ]; then
        ok "k2 app logs -- acme: the hand-over fails, naming run/cli"
    else
        bad "k2 app logs -- acme: the hand-over did not fail (exit $(cat "$WORK/bin.code"))"
        show "$WORK/bin.err"
    fi
fi

echo "== -h after a bare -- is the verb's word, answered natively =="
row req "GET /cli/organization/show" "org=-h" -- organization show -- -h
row stop 0 "No sessions yet for" -- sessions archives -- -h
row req "GET /cli/wiki/index" "" -- wiki index -- -h
row req "POST /cli/project-group/msg" '"-h"' -- project msg acme -- -h
row req "POST /cli/focus-groups/create" '"name": "-h"' -- focus-group create -- -h
row stop 2 "takes only --json" -- zen garden list -- -h
row req "POST /cli/terminal/write" "message=-h" -- \
    terminal write 123e4567-e89b-12d3-a456-426614174000 -- -h
row req "GET /cli/vault/run-helper" "" -- vault run --secret A -- grep -h
if [ "$(cat "$WORK/bash.helper")" = '["--", "grep", "-h"]' ]; then
    ok "k2 vault run --secret A -- grep -h: bash's helper runs grep -h"
else
    bad "k2 vault run --secret A -- grep -h: bash's helper ran $(cat "$WORK/bash.helper")"
fi

echo ""
echo "dashdash_help_native: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
