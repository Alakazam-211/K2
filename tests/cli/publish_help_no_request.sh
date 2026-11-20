#!/usr/bin/env bash
# -h or --help anywhere on `k2 publish` prints its help, exits 0 and sends
# nothing.
#
# Regression (0.46.17): `k2 publish subdomain claim acme -h` claimed acme,
# and `k2 publish subdomain claim -h` and `create -h --target x` took -h as
# the label: only the subcommand's own loop read help, and claim and create
# read none.
#
# Every help form below prints exactly the publish usage (on stderr, where
# `k2 publish --help` prints it), nothing on stdout, exits 0, and the stub
# daemon records no request at all. status, secret, env and subdomain list
# keep their own pages. Then the plain forms still send their request, so
# the stub is known to see one when it is sent.
#
# A subdomain label never begins with '-': `claim -x`, `unclaim -x`,
# `create -x --target x`, `point -x --target x`, `target -x x` and `rm -x`
# are a usage error (exit 2, "Error: unknown flag '-x'" on stderr) and
# send nothing. point and target refuse a dash word in either place.
#
# `--`: the router has no end-of-options rule for publish; -h after `--`
# is still help, like anywhere else on the line. claim refuses `--` itself
# as an unknown flag (as before), and create skips it as it skips any
# unknown long flag, so a dash label after it is still refused.
#
# How: a python stub daemon on a free loopback port records every request
# (any method, any path). HOME is a temp dir, no real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/publish_help_no_request.sh
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

WORK="$(mktemp -d -t k2-publish-help-XXXXXX)"
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
        out = json.dumps({"success": True, "projectId": "p1"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
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

# run <args...>: sets OUT (stdout), ERR (stderr), RC. stdin is empty, so
# a form that reads stdin can't hang.
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

# check <args...>: prints exactly the publish usage on stderr, nothing on
# stdout, exits 0, sends nothing.
check() {
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$ERR" = "$REF" ] && ok "$label: prints the publish usage" \
        || { bad "$label: stderr is not the publish usage"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ -z "$OUT" ] && ok "$label: nothing on stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
}

# page <file> <args...>: prints that help page on stdout (it starts with
# the page's first line), exits 0, sends nothing.
page() {
    local file="$1"; shift
    local label="k2 $*" first
    first="$(head -n 1 "$TEXT/$file")"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$(printf '%s\n' "$OUT" | head -n 1)" = "$first" ] && ok "$label: prints its own page ($file)" \
        || { bad "$label: stdout is not $file"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
}

# dash <args...>: a usage error, exit 2, one line on stderr naming the
# word, nothing on stdout, sends nothing.
dash() {
    local word="$1"; shift
    local label="k2 $*"
    : >"$REQ_LOG"
    run "$@"
    [ "$RC" -eq 2 ] && ok "$label: exit 2" || bad "$label: exit $RC (want 2)"
    [ "$ERR" = "Error: unknown flag '$word'" ] && ok "$label: refuses '$word'" \
        || { bad "$label: stderr"; printf '%s\n' "$ERR" | sed 's/^/      | /' >&2; }
    [ -z "$OUT" ] && ok "$label: nothing on stdout" \
        || { bad "$label: stdout"; printf '%s\n' "$OUT" | sed 's/^/      | /' >&2; }
    sent_nothing "$label"
}

echo "== the reference: k2 publish --help =="
: >"$REQ_LOG"
run publish --help
REF="$ERR"
[ "$RC" -eq 0 ] && ok "k2 publish --help: exit 0" || bad "k2 publish --help: exit $RC (want 0)"
[ "$REF" = "$(cat "$TEXT/publish.txt")" ] && ok "k2 publish --help: the publish usage" \
    || { bad "k2 publish --help: not the publish usage"; printf '%s\n' "$REF" | sed 's/^/      | /' >&2; }
sent_nothing "k2 publish --help"

echo "== subdomain claim, unclaim, create, point, rm =="
check publish subdomain claim -h
check publish subdomain claim acme -h
check publish subdomain claim acme --help
check publish subdomain claim --transfer acme -h
check publish subdomain unclaim acme -h
check publish subdomain create -h --target x
check publish subdomain create acme --target x --help
check publish subdomain create acme --target -h
check publish subdomain point acme --target x -h
check publish subdomain target acme -h
check publish subdomain rm acme -h
check publish subdomain -h
check publish -h

echo "== -h after -- is still help =="
check publish subdomain claim -- -h
check publish subdomain create --target x -- -h

echo "== a label beginning with '-' is a usage error =="
dash -x publish subdomain claim -x
dash -x publish subdomain claim --transfer -x
dash -x publish subdomain unclaim -x
dash -x publish subdomain create -x --target x
dash -x publish subdomain create --target x -x
dash -x publish subdomain create -- -x --target x
dash -- publish subdomain claim -- acme
dash -x publish subdomain point -x --target x
dash -x publish subdomain point --target x -x
dash -x publish subdomain target -x x
dash -x publish subdomain target acme -x
dash -x publish subdomain rm -x
dash -x publish subdomain remove -x
dash -x publish subdomain delete -x

echo "== pages that stay their own =="
page publish/subdomain/list.txt publish subdomain list -h
page publish/status.txt publish status acme -h
page publish/secret.txt publish secret add svc NAME -h
page vault/service.txt publish env svc -h

echo "== the plain point and rm still check their label =="
: >"$REQ_LOG"; run publish subdomain point acme --target localhost:3000
grep -q '^POST /cli/publish/subdomain/claim ' "$REQ_LOG" && ok "k2 publish subdomain point acme still checks its label" \
    || { bad "point sent no label check"; sed 's/^/      | /' "$REQ_LOG" >&2; }
: >"$REQ_LOG"; run publish subdomain rm acme
grep -q '^POST /cli/publish/subdomain/claim ' "$REQ_LOG" && ok "k2 publish subdomain rm acme still checks its label" \
    || { bad "rm sent no label check"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo "== the stub sees the plain forms' requests =="
: >"$REQ_LOG"; run publish subdomain claim acme
grep -q '^POST /cli/publish/subdomain/claim ' "$REQ_LOG" && ok "k2 publish subdomain claim acme still sends its request" \
    || { bad "claim sent no request"; sed 's/^/      | /' "$REQ_LOG" >&2; }
[ "$OUT" = "Claimed acme for this workspace (project p1)." ] && ok "k2 publish subdomain claim acme: claimed" \
    || bad "claim acme printed: $OUT / $ERR"
: >"$REQ_LOG"; run publish subdomain create staging --target localhost:3000
grep -q '^POST /cli/publish/subdomain/claim ' "$REQ_LOG" && ok "k2 publish subdomain create staging still checks its label" \
    || { bad "create sent no label check"; sed 's/^/      | /' "$REQ_LOG" >&2; }
grep -q '^POST /cli/connect/proxy .*"/subdomains"' "$REQ_LOG" && ok "k2 publish subdomain create staging still sends the create" \
    || { bad "create sent no create"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo ""
echo "publish_help_no_request: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
