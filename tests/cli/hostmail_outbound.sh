#!/usr/bin/env bash
# 0.45.1 hosted-mail field fixes — CLI wiring, no daemon, no Stalwart.
# `k2 hostmail outbound` (show / dane on), the status `outbound :` line and
# the ACME detail on the `cert :` line, and `domain show` printing the mail
# host's A row before the PTR row. A local echo server checks the exact
# requests and serves fixture JSON.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'outbound) cmd_hostmail_outbound' "$K2" || fail "cmd_hostmail must route outbound"
grep -A4 'verb == "outbound"' "$K2" | grep -q '"/cli/mail/outbound"' \
  || fail "outbound verb must use /cli/mail/outbound"

# Help + schema.
help="$("$K2" hostmail outbound --help)"
for want in 'v4Only' 'ONCE' 'never changed' 'dane on' '0.16.20' 'set by hand' \
            'k2 hostmail queue retry' 'disable/enable'; do
  printf '%s' "$help" | grep -qF -- "$want" || fail "outbound --help must mention '$want'"
done
"$K2" hostmail --help | grep -qF 'outbound [show] | outbound dane on' || fail "hostmail --help must list outbound"
schema="$("$K2" --schema)" || fail "k2 --schema exited non-zero"
grep -qF '"name": "hostmail outbound"' <<<"$schema" || fail "schema missing hostmail outbound"

expect_usage() {
  set +e
  out="$("$K2" "$@" 2>&1)"
  rc=$?
  set -e
  [ "$rc" -eq 2 ] || fail "'k2 $*' must exit 2, got $rc: $out"
}
expect_usage hostmail outbound dane off
expect_usage hostmail outbound dane
expect_usage hostmail outbound --v6

WORK="$(mktemp -d -t k2-hostmail-outbound-XXXXXX)"
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
SHOW = {"ok": True, "installedVersion": "0.16.10", "ipStrategy": "v4Only", "ipStrategySetBy": "k2",
        "dane": [{"name": "invalid-tls", "dane": "disable", "setBy": "k2"},
                 {"name": "default", "dane": "disable", "setBy": "hand"}],
        "daneSummary": "partly off", "daneAutoOff": True, "dnssec": "warn"}
POST = {"ok": True, "changed": ["invalid-tls", "default"],
        "hint": "DANE is optional again on invalid-tls, default"}
STATUS = {"ok": True, "consistent": True, "supported": True, "state": "running",
          "hostname": "mail.example.com", "lastError": "acme: connection refused",
          "cert": {"host": "mail.example.com", "names": [], "state": "missing",
                   "selfSigned": False, "expiresAt": None, "owner": "stalwart-acme",
                   "acme": {"mode": "Automatic", "orderNames": ["mail.example.com"], "locked": True,
                            "lastTask": {"id": "t1", "state": "Failed", "reason": "connection refused",
                                         "at": "2026-10-07T20:00:00Z"}}},
          "outbound": {"ipStrategy": "v4Only", "ipStrategySetBy": "k2", "daneSummary": "off (K2, until 0.16.20)",
                       "dnssec": "ok", "dane": []}}
DOMAIN = {"ok": True, "domain": "example.com", "status": "verified", "sendMode": "direct",
          "records": [
              {"id": "mx", "type": "MX", "name": "example.com", "purpose": "Mail routing (MX)",
               "expected": "10 mail.example.com.", "status": "valid"},
              {"id": "a:mail.example.com", "type": "A", "name": "mail.example.com",
               "purpose": "Mail host address — MX points here, the certificate needs it",
               "expected": "203.0.113.7", "status": "missing"},
              {"id": "ptr", "type": "PTR", "name": "mail.example.com",
               "purpose": "Reverse DNS (K2 Cloud: `k2 hostmail ptr set`; self-hosted: at your VPS provider — not in this zone)",
               "expected": "Set the reverse DNS", "status": "unverifiable"}]}
class H(http.server.BaseHTTPRequestHandler):
    def _reply(self, body, reply):
        log.write("%s %s %s\n" % (self.command, self.path.split("?")[0], body))
        log.flush()
        out = json.dumps(reply).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def do_GET(self):
        path = self.path.split("?")[0]
        reply = {"/cli/mail/status": STATUS, "/cli/mail/domain/show": DOMAIN}.get(path, SHOW)
        self._reply("-", reply)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._reply(self.rfile.read(n).decode(), POST)
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

show="$(k2 hostmail outbound)"
printf '%s' "$show" | grep -q '^mx route : v4Only (set by K2)' || fail "show mx route: $show"
printf '%s' "$show" | grep -q '^DANE     : default .*disable (set by hand)' || fail "show DANE: $show"
printf '%s' "$show" | grep -qF 'k2 hostmail outbound dane on' || fail "show must name dane on: $show"

on="$(k2 hostmail outbound dane on)"
printf '%s' "$on" | grep -qF 'DANE is optional again' || fail "dane on output: $on"
k2 hostmail outbound --json >/dev/null

status_out="$(k2 hostmail status || true)"
printf '%s' "$status_out" | grep -qF 'outbound : v4Only · DANE off (K2, until 0.16.20) · DNSSEC ok' \
  || fail "status must print the outbound line: $status_out"
printf '%s' "$status_out" | grep -qF 'cert     : missing mail.example.com — ACME failed 2026-10-07T20:00:00Z: connection refused (orders: mail.example.com only)' \
  || fail "status must print the ACME detail: $status_out"
printf '%s' "$status_out" | grep -qF 'last err : acme: connection refused' || fail "status lastError: $status_out"

dom="$(k2 hostmail domain show example.com)"
a_line="$(printf '%s\n' "$dom" | grep -n 'A mail.example.com' | head -1 | cut -d: -f1)"
ptr_line="$(printf '%s\n' "$dom" | grep -n 'PTR mail.example.com' | head -1 | cut -d: -f1)"
[ -n "$a_line" ] && [ -n "$ptr_line" ] || fail "domain show must print the A and PTR rows: $dom"
[ "$a_line" -lt "$ptr_line" ] || fail "the A row must print before the PTR row: $dom"

cat >"$WORK/check.py" <<'PY'
import json, sys
rows = [l.split(" ", 2) for l in open(sys.argv[1]).read().splitlines()]
want = [
    ("GET", "/cli/mail/outbound", None),
    ("POST", "/cli/mail/outbound", {"dane": "on"}),
    ("GET", "/cli/mail/outbound", None),
    ("GET", "/cli/mail/status", None),
    ("GET", "/cli/mail/domain/show", None),
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

echo "OK: hostmail outbound + status + domain A row CLI wiring"
