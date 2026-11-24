#!/usr/bin/env bash
# -h or --help anywhere on `k2 desktop` prints the desktop usage, exits 0
# and sends nothing, even right after a flag that takes a value.
#
# Regression (0.46.17): cmd_desktop read -h/--help only where a flag could
# start, so a value flag took it as its value and the request went out:
# `k2 desktop open --display --help` opened the desktop with --help as the
# display, `k2 desktop start --size -h` asked for a start, and the same
# held for --desktop, --timeout, --open, --workspace, --since, --limit and
# --hide-app. It now checks every word first (_wants_help, which stops at
# a bare `--`).
#
# Every help form below prints exactly what `k2 desktop --help` prints, on
# stdout, nothing on stderr, exit 0, and the stub daemon records no
# request. `-h` after `--` is not help, and desktop takes no `--`: the
# router refuses the line (exit 2) and nothing is sent. Then the plain
# forms still send their request, so the stub is known to see one when it
# is sent.
#
# How: a python stub daemon on a free loopback port records every request.
# HOME is a temp dir, no real daemon, no network. Loud.
# Run with: bash tests/cli/desktop_help_no_request.sh
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

WORK="$(mktemp -d -t k2-desktop-help-XXXXXX)"
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

# check <args...>: the desktop usage on stdout, nothing on stderr, exit 0,
# nothing sent.
check() {
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$REF" ] && ok "$label: prints the desktop usage" \
        || { bad "$label: stdout is not the desktop usage"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
}

echo "== the reference: k2 desktop --help =="
: >"$REQ_LOG"
run desktop --help
REF="$OUT"
[ "$RC" -eq 0 ] && ok "k2 desktop --help: exit 0" || bad "k2 desktop --help: exit $RC (want 0)"
[ "$REF" = "$(cli_page "$TEXT/desktop.txt")" ] && ok "k2 desktop --help: the desktop usage" \
    || { bad "k2 desktop --help: not the desktop usage"; printf '%s\n' "$REF" | sed 's/^/      | /' >&2; }
sent_nothing "k2 desktop --help"

echo "== help right after a flag that takes a value =="
check desktop open --display --help
check desktop open --display -h
check desktop start --size -h
check desktop start --desktop --help
check desktop stop --desktop -h
check desktop enable --desktop --help
check desktop disable --desktop -h
check desktop setup --timeout --help
check desktop env --timeout -h
check desktop ask --open -h
check desktop allow on --workspace --help
check desktop allow off --workspace -h
check desktop policy --hide-app --help
check desktop grant alice --desktop -h
check desktop kick dv_1 --desktop --help
check desktop audit --since -h
check desktop audit --limit --help

echo "== help anywhere else on the line =="
check desktop -h
check desktop help
check desktop open -h
check desktop start --size 1600x900 --help
check desktop env --release -h
check desktop setup --no-wait --help
check desktop ask --check -h
check desktop grant alice --view --help
check desktop revoke alice -h
check desktop kick dv_1 --help
check desktop policy --no-hide-apps -h
check desktop --json -h

echo "== -h after -- is not help: a usage error, nothing sent =="
for form in "start -- -h" "open -- --help"; do
    : >"$REQ_LOG"
    # shellcheck disable=SC2086
    run desktop $form
    label="k2 desktop $form"
    [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
    [ "$ERR" = "usage: desktop takes no '--' (k2 help desktop)" ] && ok "$label: says why" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ -z "$OUT" ] && ok "$label: nothing on stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
done

echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"
run desktop open --display 2
if grep -q '^POST /cli/desktop/open .*"display": "2"' "$REQ_LOG"; then
    ok "k2 desktop open --display 2 still sends the open"
else
    bad "k2 desktop open --display 2 sent no open (rc $RC)"
    sed 's/^/      | /' "$REQ_LOG" >&2; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
fi
: >"$REQ_LOG"
run desktop start --size 1600x900
if grep -q '^POST /cli/desktop/start .*"width": 1600' "$REQ_LOG"; then
    ok "k2 desktop start --size 1600x900 still sends the start"
else
    bad "k2 desktop start --size 1600x900 sent no start (rc $RC)"
    sed 's/^/      | /' "$REQ_LOG" >&2; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
fi

echo ""
echo "desktop_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
