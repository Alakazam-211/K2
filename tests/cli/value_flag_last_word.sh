#!/usr/bin/env bash
# A flag that takes a value, as the last word, is a usage error: exit 2,
# one line on stderr, nothing sent. It never spins.
#
# Regression (0.46.17): `k2 ci signer add --hello` (or --out) as the last
# word ran `shift 2` with one word left, which shifts nothing. And
# `k2 zen data restore <widget> --rev` looped for good: its loop ran
# `shift 2 || true`, so the flag never left the line.
#
# Now:
#   k2 ci signer add ... --hello | --out     the add usage, exit 2
#   k2 zen data restore <w> ... --rev        "Error: --rev takes a value",
#   (any of its value flags)                  exit 2
# and the stub daemon records no request. The plain `k2 ci signer list`
# and `k2 zen data restore <w> --rev 3` still send theirs, so the stub is
# known to see a request when one is sent.
#
# Every run is under a time guard (20 s): a form that spins is killed and
# fails its row, so a regression fails here instead of hanging the job.
#
# How: a python stub daemon on a free loopback port records every request.
# HOME is a temp dir, no real daemon, no network. Loud.
# Run with: bash tests/cli/value_flag_last_word.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-value-flag-XXXXXX)"
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
# The workspace has seen the one-time "Gardens changed in 0.46.3" line, so
# the first k2 zen row's stderr is only its own.
printf '0.46.3\n' >"$WORK/proj/.k2/.garden-changes-hint"
printf '%s\n' '{"hello":{"v":1}}' >"$WORK/hello.json"

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
        out = json.dumps({"controllerFp": "fp", "signers": []}).encode()
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

# run <args...>: sets OUT, ERR, RC. stdin is empty. A run past 20 s is
# killed (RC 124), so a spinning loop fails its row and the script goes on.
run() {
    local pid killer
    set +e
    "$K2" "$@" </dev/null >"$WORK/out" 2>"$WORK/err" &
    pid=$!
    ( sleep 20; kill -9 "$pid" 2>/dev/null && : >"$WORK/killed" ) &
    killer=$!
    rm -f "$WORK/killed"
    wait "$pid"
    RC=$?
    kill "$killer" 2>/dev/null
    wait "$killer" 2>/dev/null
    [ -e "$WORK/killed" ] && RC=124
    set -e
    OUT="$(cat "$WORK/out")"
    ERR="$(cat "$WORK/err")"
}

sent_nothing() {
    if [ -s "$REQ_LOG" ]; then
        bad "$1: sent a request:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$1: sent nothing"
    fi
}

# usage_err <want stderr> <args...>: exit 2, exactly that stderr, nothing
# on stdout, nothing sent.
usage_err() {
    local want="$1"; shift
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -ne 124 ] && ok "$label: ends (no spin)" || bad "$label: still running after 20 s (killed)"
    [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
    [ "$ERR" = "$want" ] && ok "$label: says why" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ -z "$OUT" ] && ok "$label: nothing on stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
}

ADD_USAGE="usage: k2 ci signer add --code-stdin --hello <pair-begin output file> [--out offer.json] < code-file (or <CODE.FP> instead of --code-stdin, visible to every account on this computer)"
H="$WORK/hello.json"

echo "== k2 ci signer add: --hello or --out as the last word =="
usage_err "$ADD_USAGE" ci signer add --hello
usage_err "$ADD_USAGE" ci signer add --out
usage_err "$ADD_USAGE" ci signer add --code-stdin --hello
usage_err "$ADD_USAGE" ci signer add --code-stdin --hello "$H" --out
usage_err "$ADD_USAGE" ci signer add --code-stdin --out x.json --hello
usage_err "$ADD_USAGE" ci signer add CODE.FP --hello "$H" --out
usage_err "$ADD_USAGE" ci signer add CODE.FP --hello

echo "== k2 zen data restore: a value flag as the last word =="
usage_err "Error: --rev takes a value" zen data restore w1 --rev
usage_err "Error: --prefix takes a value" zen data restore w1 --rev 3 --prefix
usage_err "Error: --except takes a value" zen data restore w1 --rev 3 --except
usage_err "Error: --at takes a value" zen data restore w1 --dry-run --at

echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"
run ci signer list
if grep -q '^GET /cli/ci/signers' "$REQ_LOG"; then
    ok "k2 ci signer list still asks the daemon"
else
    bad "k2 ci signer list sent no request (rc $RC)"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
fi
: >"$REQ_LOG"
run zen data restore w1 --rev 3
if grep -q '^POST /cli/zen/data/restore .*"toRev": 3' "$REQ_LOG"; then
    ok "k2 zen data restore w1 --rev 3 still sends the restore"
else
    bad "k2 zen data restore w1 --rev 3 sent no restore (rc $RC)"
    sed 's/^/      | /' "$REQ_LOG" >&2; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2
fi

echo ""
echo "value_flag_last_word: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
