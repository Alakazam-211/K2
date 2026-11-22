#!/usr/bin/env bash
# On `k2 app` commands the word `help` before a bare `--` is help, never a
# name, and a name that is literally `help` goes after the `--` (0.46.17).
#
# Regression: the verbs that read names in a loop took a trailing `help`
# as the App, request or login. `k2 app deny <id> help` denied a request
# named help; `k2 app pairing off <app> help` turned pairing off for an
# App named help. Nine of them also refused -h as an unknown flag, and
# `k2 app deny -h` printed its help on stderr.
#
# Help rows: `<line with help>` and the same line with -h in its place
# both exit 0, print the same thing, send nothing, and print the page on
# stdout with nothing on stderr.
# Name rows: `<line> -- help` gets past the `--` gate and sends `help` as
# the name (a query value, a JSON string or a path segment).
#
# How: a python stub daemon on a free loopback port records every request
# (method, path with its query, body). HOME is a temp dir, no real
# daemon, no network. Rows marked `owner` run as the owner's terminal
# (port and token on disk in their own HOME), for the verbs that refuse a
# passport before they send.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/app_help_word.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-app-help-word-XXXXXX)"
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
: >"$REQ_LOG"
mkdir -p "$WORK/proj/.k2"

python3 - "$WORK" <<'PYEOF' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, self.path, body.replace("\n", " ")))
        raw = b"{}"
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
    def do_GET(self):
        self._handle()
    def do_POST(self):
        self._handle()
    def do_PUT(self):
        self._handle()
    def do_DELETE(self):
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
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN="owner-token-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"
# Files some rows name.
echo '{}' >sa.json
echo notes >notes.txt

# The owner's terminal outside K2: no passport in the env, the port and
# the owner token on disk in another HOME.
mkdir -p "$WORK/owner/.k2"
cat "$WORK/port" >"$WORK/owner/.k2/heartbeat.port"
echo owner-token-for-test >"$WORK/owner/.k2/heartbeat.token"

# run <owner:0|1> <args...>: OUT, ERR, RC and the requests in SENT (all
# but the permission cards read).
run() {
    local owner="$1"; shift
    : >"$REQ_LOG"
    set +e
    if [ "$owner" = 1 ]; then
        OUT="$(HOME="$WORK/owner" env -u K2_PORT -u K2_HOOK_TOKEN "$K2" "$@" </dev/null 2>"$WORK/err")"
    else
        OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    fi
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
    # The app family page reads whether permission cards are on; that
    # read changes nothing and is not counted.
    SENT="$(grep -v '^GET /cli/permission/enabled ' "$REQ_LOG" || [ $? -eq 1 ])"
}

show() { printf '%s\n' "$1" | sed 's/^/      | /' >&2; }

# help_row <row>: the row is `[owner] <words of the line>`, one of them
# the word help.
help_row() {
    local owner=0 word
    local -a words=() dash=()
    set -- $1
    [ "${1:-}" = owner ] && { owner=1; shift; }
    words=("$@")
    for word in "${words[@]}"; do
        if [ "$word" = help ]; then dash+=(-h); else dash+=("$word"); fi
    done
    local label="k2 ${words[*]}"
    run "$owner" "${words[@]}"
    local h_out="$OUT" h_err="$ERR" h_rc="$RC" h_sent="$SENT"
    run "$owner" "${dash[@]}"
    [ "$h_rc" -eq 0 ] && ok "$label: exit 0" || { bad "$label: exit $h_rc (want 0)"; show "$h_out"; show "$h_err"; }
    [ "$RC" -eq 0 ] || { bad "k2 ${dash[*]}: exit $RC (want 0)"; show "$ERR"; }
    if [ -z "$h_sent" ] && [ -z "$SENT" ]; then
        ok "$label: sent nothing"
    else
        bad "$label: sent a request"; show "$h_sent"; show "$SENT"
    fi
    if [ "$h_out" = "$OUT" ] && [ "$h_err" = "$ERR" ]; then
        ok "$label: prints what -h prints"
    else
        bad "$label: prints something else than with -h"; show "$h_out$h_err"; show "$OUT$ERR"
    fi
    { [ -n "$h_out" ] && [ -z "$h_err" ]; } && ok "$label: the page on stdout, nothing on stderr" \
        || { bad "$label: want the page on stdout and nothing on stderr"; show "$h_err"; }
}

# name_row <row>: the row is `[owner] <words of the line>`, ending in
# `-- help`. Some request carries help as a name. A row marked `local`
# names a folder or file on this machine: the CLI's own answer names it.
name_row() {
    local owner=0 where=daemon
    set -- $1
    [ "${1:-}" = owner ] && { owner=1; shift; }
    [ "${1:-}" = local ] && { where=local; shift; }
    local label="k2 $*"
    run "$owner" "$@"
    case "$ERR" in
        *"takes no '--'"*) bad "$label: refused its --"; show "$ERR"; return ;;
    esac
    if [ "$where" = local ]; then
        case "$OUT$ERR" in
            *help*) ok "$label: reads help as the name (exit $RC)" ;;
            *) bad "$label: says nothing of help (exit $RC)"; show "$OUT"; show "$ERR" ;;
        esac
        return
    fi
    if grep -Eq '(=help([&# ]|$)|"help"|/help([/? ]|$))' <<<"$SENT"; then
        ok "$label: sends help as the name (exit $RC)"
    else
        bad "$label: sent no request with help as the name (exit $RC)"; show "$SENT"; show "$ERR"
    fi
}

echo "== k2 app deny -h: the help on stdout =="
run 0 app deny -h
[ "$RC" -eq 0 ] && ok "k2 app deny -h: exit 0" || bad "k2 app deny -h: exit $RC (want 0)"
case "$OUT" in
    "usage: k2 app deny "*) ok "k2 app deny -h: the usage on stdout" ;;
    *) bad "k2 app deny -h: stdout is not the usage"; show "$OUT" ;;
esac
[ -z "$ERR" ] && ok "k2 app deny -h: nothing on stderr" || { bad "k2 app deny -h: stderr"; show "$ERR"; }
[ -z "$SENT" ] && ok "k2 app deny -h: sent nothing" || { bad "k2 app deny -h: sent"; show "$SENT"; }

echo "== app-auth.sh: the word help is help =="
while IFS= read -r row; do
    [ -n "$row" ] && help_row "$row"
done <<'ROWS'
app auth shop help
app auth door on help
app auth origins shop --origin https://acme.test help
app pairing status shop help
app pairing setup shop --desktop-only help
app pairing off shop help
app pairing check shop help
app pairing log shop help
app push status shop help
app push setup shop --fcm sa.json help
app push creds rm shop --all help
app push check shop help
app push on shop help
app push off shop help
app push set shop --max-devices 3 help
app push send shop --to ada --title t --body b help
app push test shop --to ada help
app push log shop help
app push devices shop help
app push devices rm shop apd_1 help
app email show shop help
app email on shop --sender ada@acme.test help
app email off shop help
app email test shop help
app email send shop --to ada@acme.test --subject s --text t help
app email log shop help
app email status shop em_1 help
app approvals w_1 help
app approvals revoke w_1 help
owner app approve w_1 help
app deny w_1 help
app deny w_1 --reason help
app access ws help
ROWS

echo "== app-auth.sh: a name that is literally help goes after -- =="
while IFS= read -r row; do
    [ -n "$row" ] && name_row "$row"
done <<'ROWS'
app auth -- help
app pairing status -- help
app pairing setup --desktop-only -- help
app pairing off -- help
app pairing check -- help
app pairing log -- help
app push status -- help
app push setup --fcm sa.json -- help
app push creds rm --all -- help
app push check -- help
app push on -- help
app push off -- help
app push set --max-devices 3 -- help
app push send --to ada --title t --body b -- help
app push test --to ada -- help
app push log -- help
app push devices -- help
app push devices rm shop -- help
app email show -- help
app email on --sender ada@acme.test -- help
app email off -- help
app email test -- help
app email send --to ada@acme.test --subject s --text t -- help
app email log -- help
app email status shop -- help
app approvals -- help
app approvals revoke -- help
app deny -- help
app access -- help
ROWS

echo "== app approve -- help: help is the request id (the owner) =="
run 1 app approve -- help
case "$ERR" in
    *"matches 'help'"*) ok "k2 app approve -- help: looks for the request help (exit $RC)" ;;
    *) bad "k2 app approve -- help: stderr does not name the request help (exit $RC)"; show "$ERR" ;;
esac
grep -q '^GET /cli/app/approvals' <<<"$SENT" && ok "k2 app approve -- help: reads the approvals" \
    || { bad "k2 app approve -- help: did not read the approvals"; show "$SENT"; }

echo ""
echo "app_help_word: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
