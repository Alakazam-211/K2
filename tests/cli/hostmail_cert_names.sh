#!/usr/bin/env bash
# hostmail cert names (CAL44: one non-default certificate per extra mail
# name) CLI wiring — no daemon, no Stalwart, no ACME. Static wiring + help +
# schema + usage errors, then a local echo server checks the exact requests
# (GET list, POST issue/renew bodies), the human output, the exit code when
# a name failed, and the status `names` line.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'names) cmd_hostmail_cert_names' "$K2" || fail "cmd_hostmail_cert must route names"
grep -A12 'verb == "cert_names"' "$K2" | grep -q '"/cli/mail/cert/names"' \
  || fail "cert_names verb must use /cli/mail/cert/names"

# Help.
help="$("$K2" hostmail cert names --help)"
for want in 'autoconfig' 'autodiscover' 'mta-sts' 'ua-auto-config' 'never the apex' \
            'never' 'ReloadTlsCertificates' 'left to expire' 'K2-hosted' 'Owner/admin' \
            'names [list]' 'names issue' 'names renew' '--name <host>'; do
  printf '%s' "$help" | grep -qF -- "$want" || fail "cert names --help must mention '$want'"
done
printf '%s' "$help" | grep -q 'disable/enable' || fail "cert names --help must say never hostmail disable/enable"
"$K2" hostmail cert --help | grep -q 'cert names' || fail "cert --help must list cert names"
"$K2" hostmail --help | grep -q 'cert names \[issue|renew\]' || fail "hostmail --help must list cert names"

# Schema.
schema="$("$K2" --schema)" || fail "k2 --schema exited non-zero"
for n in "hostmail cert names" "hostmail cert names issue" "hostmail cert names renew"; do
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
expect_usage hostmail cert wipe
expect_usage hostmail cert names wipe
expect_usage hostmail cert names issue --name
expect_usage hostmail cert names --name autoconfig.example.com
expect_usage hostmail cert names list --name autoconfig.example.com
expect_usage hostmail cert names renew extra

# Live: a local echo server records method, path and body.
WORK="$(mktemp -d -t k2-hostmail-cert-names-XXXXXX)"
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
LIST = {"ok": True, "live": True, "mailHost": "mail.example.com",
        "box": {"v4": ["192.0.2.10"], "v6": [], "source": "public-ip"},
        "names": [
          {"name": "autoconfig.example.com", "apex": "example.com", "pointsHere": True,
           "dns": None, "issuer": "k2-dns-01", "reason": None,
           "cert": {"state": "missing", "expiresAt": None}},
          {"name": "mta-sts.example.com", "apex": "example.com", "pointsHere": False,
           "dns": "mta-sts.example.com A 192.0.2.99 is not this box (192.0.2.10)",
           "issuer": "k2-dns-01", "reason": None,
           "cert": {"state": "missing", "expiresAt": None}}]}
RUN = {"ok": True, "failed": 1, "reload": {"method": "action", "error": None},
       "names": [
         {"name": "autoconfig.example.com", "result": "issued", "expiresAt": 1900000000,
          "detail": None},
         {"name": "autodiscover.example.com", "result": "failed", "expiresAt": None,
          "detail": "issue: acme order failed"}]}
STATUS = {"ok": True, "consistent": True, "supported": True, "state": "running",
          "hostname": "mail.example.com",
          "extraNames": [
            {"name": "autoconfig.example.com", "pointsHere": True,
             "cert": {"state": "issued", "expiresAt": 1900000000}},
            {"name": "autodiscover.example.com", "pointsHere": True,
             "cert": {"state": "missing", "expiresAt": None}},
            {"name": "mta-sts.example.com", "pointsHere": False,
             "cert": {"state": "missing", "expiresAt": None}}]}
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
        self._reply("-", STATUS if path == "/cli/mail/status" else LIST)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._reply(self.rfile.read(n).decode(), RUN)
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

human="$(k2 hostmail cert names)"
printf '%s' "$human" | grep -q '^autoconfig.example.com .*here yes .*cert missing' \
  || fail "list human output: $human"
printf '%s' "$human" | grep -q '192.0.2.99 is not this box' || fail "list must print why a name is skipped: $human"

set +e
run_out="$(k2 hostmail cert names issue)"
rc=$?
set -e
[ "$rc" -eq 1 ] || fail "issue with a failed name must exit 1, got $rc: $run_out"
printf '%s' "$run_out" | grep -q '^autoconfig.example.com .*issued' || fail "issue output: $run_out"
printf '%s' "$run_out" | grep -q 'acme order failed' || fail "issue output must carry the failure: $run_out"
printf '%s' "$run_out" | grep -q '^reload   : action' || fail "issue output must show the reload method: $run_out"
set +e
k2 hostmail cert names renew --name autoconfig.example.com --json >/dev/null
set -e

status_out="$(k2 hostmail status || true)"
printf '%s' "$status_out" | grep -q '^names    : 2 of 3 extra names point here; no own cert: autodiscover.example.com' \
  || fail "status must summarize extraNames: $status_out"

cat >"$WORK/check.py" <<'PY'
import json, sys
rows = [l.split(" ", 2) for l in open(sys.argv[1]).read().splitlines()]
want = [
    ("GET", "/cli/mail/cert/names", None),
    ("POST", "/cli/mail/cert/names", {"action": "issue"}),
    ("POST", "/cli/mail/cert/names", {"action": "renew", "name": "autoconfig.example.com"}),
    ("GET", "/cli/mail/status", None),
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

echo "OK: hostmail cert names CLI wiring"
