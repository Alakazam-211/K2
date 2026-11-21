#!/usr/bin/env bash
# Help never acts on `k2 permission`: -h or --help anywhere before `--`
# prints the usage (the same page as `k2 permission --help`) on stdout,
# exits 0, and sends no request but the family's own probe
# (GET /cli/permission/enabled, which `k2 permission --help` sends too).
#
# Regression (0.46.17), each found by the k5 help lane:
#   - `k2 permission status --help` asked for the status of `--help`.
#   - `k2 permission cancel --help` sent a cancel for the id `--help`.
#   - `k2 permission pending --help` asked for the pending list.
#   - `k2 permission approve --help` asked for the pending list, then
#     said no request `--help` waits (exit 4).
#   - `k2 permission beta on --help` turned permission cards on.
#   - `k2 permission beta status --help` asked for the settings.
#   - `k2 permission approvers --help` set the approvers to `--help`.
#
# Each help row: exactly the usage on stdout, nothing on stderr, exit 0,
# only the probe sent. Each verb's plain form then still sends its
# request, so the stub is known to see one.
#
# How: a person's terminal outside K2 (the owner token and port on disk
# in a temp HOME, no passport) and a python stub daemon on a free
# loopback port that records every request. No real daemon, no network.
# Loud: every failure is named and the script exits 1.
# Run with: bash tests/cli/permission_help_never_acts.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
for v in $(env | grep -E '^K[25](SO)?_' | cut -d= -f1); do unset "$v"; done

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-permission-help-XXXXXX)"
STUB_PID=""
cleanup() {
    if [ -n "${STUB_PID:-}" ]; then
        kill "$STUB_PID" 2>/dev/null || true
        wait "$STUB_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

REQ_LOG="$WORK/requests.log"
: >"$REQ_LOG"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
ANSWERS = {
    "/cli/permission/enabled": {"enabled": True},
    "/cli/permission/pending": {"requests": []},
    "/cli/settings/get": {"settings": {"permissionCards": True, "permissionApprovers": []}},
}
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        raw = json.dumps(ANSWERS.get(path, {"ok": True})).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
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
export HOME="$WORK/home"
mkdir -p "$HOME/.k2" "$WORK/proj"
cat "$WORK/port" >"$HOME/.k2/heartbeat.port"
echo owner-token-for-test >"$HOME/.k2/heartbeat.token"
cd "$WORK/proj"

run() {
    : >"$REQ_LOG"
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}

show() { printf '%s\n' "$1" | sed 's/^/      | /' >&2; }

# Nothing in the log but the family's probe.
only_probe() {
    if grep -qvE '^GET /cli/permission/enabled ' "$REQ_LOG"; then
        bad "$1: sent more than the probe:"; sed 's/^/      | /' "$REQ_LOG" >&2
    else
        ok "$1: sent nothing but the probe"
    fi
}

echo "== the page: k2 permission --help =="
run permission --help
PAGE="$OUT"
[ "$RC" -eq 0 ] && ok "k2 permission --help: exit 0" || bad "k2 permission --help: exit $RC"
case "$PAGE" in
    "Usage: k2 permission <command> (Beta)"*) ok "k2 permission --help: the usage" ;;
    *) bad "k2 permission --help: not the usage"; show "$PAGE"; show "$ERR" ;;
esac
only_probe "k2 permission --help"

# page <args...>: exactly the usage on stdout, nothing on stderr, exit 0,
# nothing sent but the probe.
page() {
    local label="k2 $*"
    run "$@"
    [ "$RC" -eq 0 ] && ok "$label: exit 0" || bad "$label: exit $RC (want 0)"
    [ "$OUT" = "$PAGE" ] && ok "$label: prints the usage" \
        || { bad "$label: stdout is not the usage"; show "$OUT"; }
    [ -z "$ERR" ] && ok "$label: nothing on stderr" \
        || { bad "$label: stderr"; show "$ERR"; }
    only_probe "$label"
}

# sends <pattern> <args...>: the plain form sends a request matching
# <pattern> (grep -E).
sends() {
    local pat="$1"; shift
    local label="k2 $*"
    run "$@"
    if grep -Eq -- "$pat" "$REQ_LOG"; then
        ok "$label: sends $pat"
    else
        bad "$label: never sent $pat:"; sed 's/^/      | /' "$REQ_LOG" >&2; show "$ERR"
    fi
}

echo "== permission status =="
# native: permission status --help
page permission status --help
page permission status pr_1 -h
sends '^GET /cli/permission/status ' permission status pr_1

echo "== permission cancel =="
# native: permission cancel --help
page permission cancel --help
page permission cancel pr_1 --help
sends '^POST /cli/permission/cancel .*"id": "pr_1"' permission cancel pr_1

echo "== permission pending =="
# native: permission pending --help
page permission pending --help
page permission pending -h
sends '^GET /cli/permission/pending ' permission pending

echo "== permission approve and deny =="
# native: permission approve --help
page permission approve --help
page permission approve pr_1 -h
page permission deny pr_1 --reason --help
sends '^GET /cli/permission/pending ' permission approve pr_1

echo "== permission beta =="
# native: permission beta --help
page permission beta on --help
page permission beta -h on
page permission beta off --help
page permission beta status --help
page permission beta --help
sends '^POST /cli/settings/update .*"permissionCards":true' permission beta on
sends '^GET /cli/settings/get ' permission beta status

echo "== permission approvers =="
# native: permission approvers --help
page permission approvers --help
page permission approvers ada --help
sends '^POST /cli/settings/update .*"permissionApprovers": \["ada"\]' permission approvers ada

echo "== the other verbs: a value flag never takes the help word =="
page permission update pr_1 --why -h
page permission list --area --help
page permission agent ws on --help
page permission request compute.use --why -h

echo ""
echo "permission_help_never_acts: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
