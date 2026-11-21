#!/usr/bin/env bash
# `k2 api off` says so when the public API stays on.
#
# 0.46.17: `k2 api off` saved apiEnabled=false and printed "Public /v1 API
# setting: OFF" even when K2_API or K2_SANDBOX_API in the daemon's
# environment kept the surface on. Now, after the settings post, it reads
# the surface the way `k2 api status` does (boot-status `api.enabled`):
#   - still on  -> one line on stderr naming the env flags, exit 1, no OFF line;
#   - really off -> today's OFF line, exit 0, no note;
#   - unknown (boot-status without `api`) -> the OFF line plus the old note.
# `k2 api on` is unchanged and never asks boot-status.
#
# How: a python stub daemon on a free loopback port records every request;
# boot-status answers from a file the test writes. HOME is a temp dir, no
# real daemon, no network. Loud: every failure is named, exit 1.
# Run with: bash tests/cli/api_off_forced_on.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_UNDER_TEST"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

WORK="$(mktemp -d -t k2-api-off-XXXXXX)"
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
printf '%s' '{}' >"$WORK/boot.json"

python3 - "$WORK" <<'PYEOF' &
import json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
work = sys.argv[1]
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def _reply(self, raw):
        body = raw.encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        path = urllib.parse.urlparse(self.path).path
        with open(work + "/requests.log", "a") as f:
            f.write("%s %s %s\n" % (self.command, path, body))
        if path == "/cli/settings/update":
            return self._reply(json.dumps({"apiEnabled": json.loads(body).get("apiEnabled")}))
        if path == "/boot-status":
            with open(work + "/boot.json") as f:
                return self._reply(f.read())
        self._reply("{}")
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

run() {
    set +e
    OUT="$("$K2" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}
OFF_LINE="Public /v1 API setting: OFF (live now — no restart needed)"
STILL_ON="The public API is still ON: K2_API or K2_SANDBOX_API is set in the daemon's environment and forces it on. The setting is saved as off; to turn the API off, remove that variable from the daemon's service and restart it."
NOTE="Note: where the K2_API (or legacy K2_SANDBOX_API) env flag is set, it still forces the surface on; check with: k2 api status"
show() { printf 'stdout: %s\nstderr: %s\n' "$OUT" "$ERR" | sed 's/^/      | /' >&2; }

echo "== forced on: an env flag keeps the surface up =="
printf '%s' '{"api":{"enabled":true}}' >"$WORK/boot.json"
: >"$REQ_LOG"
run api off
[ "$RC" -eq 1 ] && ok "api off, forced on: exit 1" || { bad "api off, forced on: exit $RC (want 1)"; show; }
[ "$ERR" = "$STILL_ON" ] && ok "api off, forced on: the still-ON line on stderr" || { bad "api off, forced on: stderr is not the still-ON line"; show; }
[ -z "$OUT" ] && ok "api off, forced on: no OFF line" || { bad "api off, forced on: printed on stdout"; show; }
grep -q '^POST /cli/settings/update {"apiEnabled":false}$' "$REQ_LOG" && ok "api off, forced on: the setting is still saved" || { bad "api off, forced on: no settings post"; sed 's/^/      | /' "$REQ_LOG" >&2; }
grep -q '^GET /boot-status ' "$REQ_LOG" && ok "api off, forced on: it read boot-status" || { bad "api off, forced on: never read boot-status"; sed 's/^/      | /' "$REQ_LOG" >&2; }

echo "== really off =="
printf '%s' '{"api":{"enabled":false}}' >"$WORK/boot.json"
run api off
[ "$RC" -eq 0 ] && ok "api off, really off: exit 0" || { bad "api off, really off: exit $RC (want 0)"; show; }
[ "$OUT" = "$OFF_LINE" ] && ok "api off, really off: today's OFF line, no note" || { bad "api off, really off: stdout is not the OFF line alone"; show; }
[ -z "$ERR" ] && ok "api off, really off: nothing on stderr" || { bad "api off, really off: stderr"; show; }

echo "== boot-status says nothing about the API =="
printf '%s' '{"ok":true}' >"$WORK/boot.json"
run api off
[ "$RC" -eq 0 ] && ok "api off, unknown: exit 0" || { bad "api off, unknown: exit $RC (want 0)"; show; }
[ "$OUT" = "$OFF_LINE"$'\n'"$NOTE" ] && ok "api off, unknown: the OFF line and the note" || { bad "api off, unknown: stdout"; show; }

echo "== api on is unchanged =="
printf '%s' '{"api":{"enabled":true}}' >"$WORK/boot.json"
: >"$REQ_LOG"
run api on
[ "$RC" -eq 0 ] && ok "api on: exit 0" || { bad "api on: exit $RC (want 0)"; show; }
[ "$OUT" = "Public /v1 API setting: ON (live now — no restart needed)" ] && ok "api on: the ON line" || { bad "api on: stdout"; show; }
grep -q '^GET /boot-status ' "$REQ_LOG" && { bad "api on asked boot-status"; sed 's/^/      | /' "$REQ_LOG" >&2; } || ok "api on: no boot-status read"

echo
echo "api_off_forced_on: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
