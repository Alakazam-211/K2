#!/usr/bin/env bash
# -h or --help anywhere on `k2 ci signer` prints the ci usage, exits 0 and
# sends nothing.
#
# Regression (0.46.17): cmd_ci read only the word after `signer` for help,
# so `k2 ci signer remove <fp> -h` removed the signer, and `finish <id>
# --done <file> -h` and `reveal <id> --reveal <file> -h` sent theirs. It
# now checks every word first (_wants_help, which stops at a bare `--`).
#
# Every help form below prints exactly what `k2 ci --help` prints, on
# stdout, nothing on stderr, exit 0, and the stub daemon records no
# request. A `-h` after `--` is not help, and no signer verb takes a `--`:
# the line is a usage error, "usage: ci signer takes no '--' (k2 help ci)"
# on stderr, exit 2, nothing sent. Then the plain remove and list still
# send theirs, so the stub is known to see a request when one is sent.
#
# How: a python stub daemon on a free loopback port records every request.
# HOME is a temp dir, no real daemon, no network. Loud.
# Run with: bash tests/cli/ci_signer_help_no_request.sh
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

WORK="$(mktemp -d -t k2-ci-signer-help-XXXXXX)"
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
        out = json.dumps({"controllerFp": "fp", "signers": [], "signer": "s", "label": "l", "pairing": "p1", "sas": "123456"}).encode()
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
# The owner's own terminal: the port and token come from HOME/.k2, not a
# session's environment, so the person-only signer verbs may send.
unset K2_PORT K2_HOOK_TOKEN
mkdir -p "$HOME/.k2"
cat "$WORK/port" >"$HOME/.k2/heartbeat.port"
printf 'owner-token-for-test\n' >"$HOME/.k2/heartbeat.token"
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

# check <args...>: the ci usage on stdout, nothing on stderr, exit 0,
# nothing sent.
check() {
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$REF" ] && ok "$label: prints the ci usage" \
        || { bad "$label: stdout is not the ci usage"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
}

printf '%s\n' '{"v":1}' >"$WORK/f.json"
F="$WORK/f.json"

echo "== the reference: k2 ci --help =="
: >"$REQ_LOG"
run ci --help
REF="$OUT"
[ "$RC" -eq 0 ] && ok "k2 ci --help: exit 0" || bad "k2 ci --help: exit $RC (want 0)"
[ "$REF" = "$(cat "$TEXT/ci.txt")" ] && ok "k2 ci --help: the ci usage" \
    || { bad "k2 ci --help: not the ci usage"; printf '%s\n' "$REF" | sed 's/^/      | /' >&2; }
sent_nothing "k2 ci --help"

echo "== help anywhere on k2 ci signer =="
check ci signer remove FP1 -h
check ci signer remove FP1 --help
check ci signer rm FP1 -h
check ci signer finish p1 --done "$F" -h
check ci signer finish p1 -h
check ci signer finish p1 --done "$F" --help
check ci signer reveal p1 --reveal "$F" -h
check ci signer reveal p1 -h
check ci signer reveal p1 --reveal "$F" --help
check ci signer add --code-stdin --hello "$F" -h
check ci signer add CODE.FP --hello "$F" --out o.json --help
check ci signer list -h
check ci signers --help
check ci signer -h

echo "== -h after -- is not help, and signer takes no --: a usage error, nothing sent =="
for form in "remove FP1 -- -h" "remove FP1 -- --help" "finish p1 --done $F -- -h" "reveal p1 --reveal $F -- --help" "list -- -h" "remove -- FP1"; do
    : >"$REQ_LOG"
    # shellcheck disable=SC2086
    run ci signer $form
    label="k2 ci signer $form"
    [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
    [ "$ERR" = "usage: ci signer takes no '--' (k2 help ci)" ] && ok "$label: says why" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ -z "$OUT" ] && ok "$label: nothing on stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
done

echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"
run ci signer remove FP1
if grep -q '^POST /cli/ci/signer/remove .*FP1' "$REQ_LOG"; then
    ok "k2 ci signer remove FP1 still sends the remove"
else
    bad "k2 ci signer remove FP1 sent no remove (rc $RC)"
    sed 's/^/      | /' "$REQ_LOG" >&2; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
fi
: >"$REQ_LOG"
run ci signer list
if grep -q '^GET /cli/ci/signers' "$REQ_LOG"; then
    ok "k2 ci signer list still asks the daemon"
else
    bad "k2 ci signer list sent no request (rc $RC)"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
fi

echo ""
echo "ci_signer_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
