#!/usr/bin/env bash
# hostmail calendar (calendars S2: DAV policy) CLI wiring — no daemon, no
# Stalwart. Static wiring + help + schema + usage errors, then a local
# echo server checks the exact requests (GET status, POST enable/disable
# bodies) and the human output.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'calendar) cmd_hostmail_calendar' "$K2" || fail "cmd_hostmail must route calendar"
grep -A12 'verb == "dav"' "$K2" | grep -q '"/cli/mail/dav"' \
  || fail "dav verb must use /cli/mail/dav"

# Help.
help="$("$K2" hostmail calendar --help)"
for want in 'calendar status' 'calendar enable' 'calendar disable' '--files on|off' \
            'k2 hostmail bans' 'allowlist' 'uninstall' 'Owner/admin'; do
  printf '%s' "$help" | grep -qF -- "$want" || fail "calendar --help must mention '$want'"
done
printf '%s' "$help" | grep -q 'never uses' || fail "calendar --help must say it never uses hostmail disable/enable"
"$K2" hostmail calendar status --help | grep -q '/cli/mail/dav' || fail "status --help must name the route"
"$K2" hostmail calendar enable --help | grep -q 'files' || fail "enable --help must explain --files"
"$K2" hostmail calendar disable --help | grep -q 'files' || fail "disable --help must explain --files"
"$K2" hostmail --help | grep -q 'calendar status|enable|disable' || fail "hostmail --help must list calendar"
"$K2" hostmail autoconfig --help | grep -q '_caldavs._tcp' || fail "autoconfig --help must mention CalDAV SRV"
"$K2" study mail | grep -q 'k2 hostmail calendar' || fail "study mail must list hostmail calendar as owner-only"

# Schema.
schema="$("$K2" --schema)" || fail "k2 --schema exited non-zero"
for n in "hostmail calendar status" "hostmail calendar enable" "hostmail calendar disable"; do
  grep -qF "\"name\": \"$n\"" <<<"$schema" || fail "schema missing $n"
done

# Usage errors exit 2 before any request.
expect_usage() {
  set +e
  out="$("$K2" "$@" 2>&1)"
  rc=$?
  set -e
  [ "$rc" -eq 2 ] || fail "'k2 $*' must exit 2, got $rc: $out"
}
expect_usage hostmail calendar wipe
expect_usage hostmail calendar enable --files maybe
expect_usage hostmail calendar enable --files
expect_usage hostmail calendar status --files off
expect_usage hostmail calendar disable extra

# Live: a local echo server records method, path and body.
WORK="$(mktemp -d -t k2-hostmail-calendar-XXXXXX)"
SRV_PID=""
cleanup() {
  if [ -n "$SRV_PID" ]; then
    kill "$SRV_PID" 2>/dev/null || true
    wait "$SRV_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
  hermetic_cli_cleanup
}
trap cleanup EXIT

cat >"$WORK/srv.py" <<'PY'
import http.server, json, sys
log = open(sys.argv[1], "a")
REPLY = {"ok": True, "enabled": True, "files": False, "policy": "applied",
         "appliedAt": 1, "backfilledAt": 1, "reachable": False,
         "url": "https://mail.example.com/.well-known/caldav",
         "reason": "your proxy must forward /.well-known/caldav",
         "accounts": {"total": 2, "updated": 2, "unchanged": 0, "skipped": [], "failed": []}}
class H(http.server.BaseHTTPRequestHandler):
    def _reply(self, body):
        log.write("%s %s %s\n" % (self.command, self.path.split("?")[0], body))
        log.flush()
        out = json.dumps(REPLY).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def do_GET(self):
        self._reply("-")
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._reply(self.rfile.read(n).decode())
    def log_message(self, *a):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
PY
: >"$WORK/port"
: >"$WORK/requests"
python3 "$WORK/srv.py" "$WORK/requests" >"$WORK/port" &
SRV_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done
[ -s "$WORK/port" ] || fail "echo server never reported its port"
PORT="$(cat "$WORK/port")"

k2() {
  env -i PATH="$PATH" HOME="$K2_TEST_HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token \
    "$K2" "$@"
}

human="$(k2 hostmail calendar status)"
printf '%s' "$human" | grep -q '^calendars : on' || fail "status human output: $human"
printf '%s' "$human" | grep -q '^files     : off' || fail "status human output: $human"
printf '%s' "$human" | grep -q 'mail.example.com/.well-known/caldav' || fail "status must print the url: $human"
printf '%s' "$human" | grep -q 'bans' || fail "status must carry the auth-ban note: $human"
k2 hostmail calendar enable --files on --json >/dev/null
k2 hostmail calendar disable >/dev/null

cat >"$WORK/check.py" <<'PY'
import json, sys
rows = [l.split(" ", 2) for l in open(sys.argv[1]).read().splitlines()]
want = [
    ("GET", "/cli/mail/dav", None),
    ("POST", "/cli/mail/dav", {"action": "enable", "files": "on"}),
    ("POST", "/cli/mail/dav", {"action": "disable"}),
]
if len(rows) != len(want):
    sys.exit("FAIL: expected %d requests, got %r" % (len(want), rows))
for (m, p, b), (wm, wp, wb) in zip(rows, want):
    if (m, p) != (wm, wp):
        sys.exit("FAIL: expected %s %s, got %s %s" % (wm, wp, m, p))
    if wb is not None and json.loads(b) != wb:
        sys.exit("FAIL: %s %s body must be %r, got %s" % (m, p, wb, b))
PY
python3 "$WORK/check.py" "$WORK/requests"

echo "OK: hostmail calendar CLI wiring"
