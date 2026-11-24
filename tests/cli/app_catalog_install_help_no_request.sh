#!/usr/bin/env bash
# -h or --help anywhere on `k2 app install --catalog` prints the catalog
# help, exits 0 and sends nothing, even right after a flag that takes a
# value. A value flag as the last word is a usage error.
#
# Regression (0.46.17): cmd_app_install_catalog read the word after
# --address (and --catalog, --id, --variant, --version, --workspace,
# --domain) as its value whatever it was, so `k2 app install --catalog x
# --address --help` sent the install with --help as the address. A value
# flag as the last word stopped on a `shift 2` that couldn't shift (exit
# 1, no message).
#
# Every row has --no-wait, so a CLI that does send the install (the fix
# reverted) ends at once instead of watching the job.
#
# Every help form below prints exactly cli/text/help/app/catalog.txt on
# stdout, nothing on stderr, exits 0, and the stub daemon records no
# request. A value flag with no word after it prints "usage: <flag> needs
# a value (k2 app catalog --help)" on stderr, exit 2, nothing sent. Then
# the plain install still sends, so the stub is known to see a request.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/app_catalog_install_help_no_request.sh
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

WORK="$(mktemp -d -t k2-app-catalog-help-XXXXXX)"
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
        out = json.dumps({"job": {"id": "cj-7", "state": "running"}}).encode()
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

REF="$(cli_page "$TEXT/app/catalog.txt")"

run() {
    : >"$REQ_LOG"
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

# check <args...>: the catalog help on stdout, nothing on stderr, exit 0,
# nothing sent.
check() {
    local label="k2 $*"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$REF" ] && ok "$label: prints the catalog help" \
        || { bad "$label: stdout is not the catalog help"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
}

# last <flag> <args...>: the flag as the last word is a usage error.
last() {
    local flag="$1"; shift
    local label="k2 $*"
    run "$@"
    [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
    [ "$ERR" = "usage: $flag needs a value (k2 app catalog --help)" ] && ok "$label: names $flag" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ -z "$OUT" ] && ok "$label: nothing on stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
}

echo "== help right after a flag that takes a value =="
check app install --catalog x --no-wait --address --help
check app install --no-wait --catalog x --address -h
check app install --no-wait --catalog x --id --help
check app install --no-wait --catalog x --variant -h
check app install --no-wait --catalog x --version --help
check app install --no-wait --catalog x --workspace -h
check app install --no-wait --catalog x --domain --help
check app install --no-wait --catalog -h
check app install --no-wait --catalog --help x

echo "== help anywhere else =="
check app install --no-wait --catalog x -h
check app install --no-wait --catalog x --dry-run --help
check app install --no-wait -h --catalog x
check app install --catalog=x --address git --no-wait -h
check app install --no-wait --catalog x myid --help

echo "== a value flag as the last word is a usage error =="
last --catalog app install --catalog
last --address app install --catalog x --address
last --id app install --catalog x --id
last --variant app install --catalog x --variant
last --version app install --catalog x --version
last --workspace app install --catalog x --workspace
last --domain app install --catalog x --domain

echo "== the plain install still sends =="
run app install --catalog x --address git --no-wait
grep -q '^POST /cli/app/catalog/install ' "$REQ_LOG" && ok "k2 app install --catalog x --address git --no-wait sends the install" \
    || { bad "the install sent no request"; sed 's/^/      | /' "$REQ_LOG" >&2; }
grep -q '"address": *"git"' "$REQ_LOG" && ok "the install carries the address" \
    || { bad "the install's body"; sed 's/^/      | /' "$REQ_LOG" >&2; }
[ "$RC" -eq 0 ] && ok "the install: exit 0" || { bad "the install: exit $RC"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }

echo ""
echo "app_catalog_install_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
