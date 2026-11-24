#!/usr/bin/env bash
# -h or --help anywhere on `k2 inbox` prints the inbox help, exits 0 and
# sends nothing.
#
# Regression (0.46.17): cmd_inbox read -h/--help only as the verb, so
# `k2 inbox delete <id> -h` deleted the item, `archive <id> -h` and
# `move <id> <folder> -h` sent theirs, and `compose --title <t> -h`
# skipped the word and composed (`compose --title --help` used it as the
# title). It now checks every word first (_wants_help, which stops at a
# bare `--`).
#
# Every help form below prints exactly what `k2 inbox --help` prints, on
# stdout, nothing on stderr, exit 0, and the stub daemon records no
# request. After `--` the words are respond's id and text, as typed (inbox
# respond is free text): `k2 inbox respond i1 -- --help` sends the text
# --help. Then the plain forms still send theirs, so the stub is known to
# see a request when one is sent.
#
# How: a python stub daemon on a free loopback port records every request.
# HOME is a temp dir, no real daemon, no network. Loud.
# Run with: bash tests/cli/inbox_help_no_request.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env
TEXT="$SCRIPT_DIR/../../cli/text/help"

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-inbox-help-XXXXXX)"
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
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        out = json.dumps({"ok": True, "state": "running"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
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
export K2_PORT="$(cat "$WORK/port")"
export K2_HOOK_TOKEN="owner-token-for-test"
export K2_PROJECT_PATH="$WORK/proj"
cd "$WORK/proj"

# run <args...>: sets OUT (stdout), ERR (stderr), RC. stdin is empty.
run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

sent_nothing() {
    if [ -s "$REQ_LOG" ]; then
        bad "$1: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$1: sent nothing"
    fi
}

# check <args...>: the inbox help on stdout, nothing on stderr, exit 0,
# nothing sent.
check() {
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$REF" ] && ok "$label: prints the inbox help" \
        || { bad "$label: stdout is not the inbox help"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
}

echo "== the reference: k2 inbox --help =="
: >"$REQ_LOG"
run inbox --help
REF="$OUT"
[ "$RC" -eq 0 ] && ok "k2 inbox --help: exit 0" || bad "k2 inbox --help: exit $RC (want 0)"
[ "$REF" = "$(cli_page "$TEXT/inbox.txt")" ] && ok "k2 inbox --help: the inbox help" \
    || { bad "k2 inbox --help: not the inbox help"; printf '%s\n' "$REF" | sed 's/^/      | /' >&2; }
sent_nothing "k2 inbox --help"

echo "== help anywhere on k2 inbox =="
check inbox compose --title T -h
check inbox compose --title T --body B --help
check inbox compose --title --help
check inbox compose -h
check inbox respond i1 hello -h
check inbox respond i1 --help
check inbox move i1 projects -h
check inbox move i1 projects --help
check inbox archive i1 -h
check inbox archive i1 --help
check inbox delete i1 -h
check inbox delete i1 --help
check inbox list -h
check inbox read i1 --help
check inbox -h

echo "== after -- the words are respond's text, as typed =="
: >"$REQ_LOG"
run inbox respond i1 -- --help
[ "$RC" -eq 0 ] && ok "k2 inbox respond i1 -- --help: exit 0" || bad "k2 inbox respond i1 -- --help: exit $RC (want 0)"
if grep -q '^POST /cli/inbox/respond .*id=i1.*text=--help' "$REQ_LOG"; then
    ok "k2 inbox respond i1 -- --help: sends the text --help"
else
    bad "k2 inbox respond i1 -- --help: --help was not sent as the text"
    sed 's/^/      | /' "$REQ_LOG" >&2; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
fi
case "$OUT" in
    "$REF"*) bad "k2 inbox respond i1 -- --help: printed the help" ;;
    *) ok "k2 inbox respond i1 -- --help: not the help" ;;
esac

echo "== the stub sees the plain forms' requests =="
for form in "delete i1|/cli/inbox/delete" "archive i1|/cli/inbox/archive" "move i1 projects|/cli/inbox/move" "compose --title T|/cli/inbox/compose"; do
    args="${form%%|*}"; path="${form#*|}"
    : >"$REQ_LOG"
    # shellcheck disable=SC2086
    run inbox $args
    if grep -q "^POST $path " "$REQ_LOG"; then
        ok "k2 inbox $args still sends it"
    else
        bad "k2 inbox $args sent nothing (rc $RC)"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
    fi
done

echo ""
echo "inbox_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
