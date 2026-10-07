#!/usr/bin/env bash
# hostmail cert owner (who owns the mail certificate: k2 | stalwart-acme)
# CLI wiring — no daemon, no Stalwart. Static wiring + help + schema + usage
# errors, then a local echo server checks the exact requests (GET show,
# POST {action: stalwart-acme}), the human output, and the status owner line.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'owner) cmd_hostmail_cert_owner' "$K2" || fail "cmd_hostmail_cert must route owner"
grep -A4 'verb == "cert_owner"' "$K2" | grep -q '"/cli/mail/cert/owner"' \
  || fail "cert_owner verb must use /cli/mail/cert/owner"

# Help.
help="$("$K2" hostmail cert owner --help)"
for want in 'k2 ' 'stalwart-acme' 'unknown' '--stalwart-acme' 'role mail' 'planted' \
            'k2 domain name remove' 'mail host only' 'never flips it' 'Owner/admin'; do
  printf '%s' "$help" | grep -qF -- "$want" || fail "cert owner --help must mention '$want'"
done
printf '%s' "$help" | grep -q 'disable/enable' || fail "cert owner --help must say never hostmail disable/enable"
cert_help="$("$K2" hostmail cert --help)"
printf '%s' "$cert_help" | grep -q 'cert owner \[--stalwart-acme\]' || fail "cert --help must list cert owner"
printf '%s' "$cert_help" | grep -qF 'cert.owner' || fail "cert --help must point at cert.owner"
printf '%s' "$cert_help" | grep -qF 'k2 study mail' || fail "cert --help must point at k2 study mail"
printf '%s' "$cert_help" | grep -qF 'never turn it back on' || fail "cert --help: never turn Stalwart ACME back on"
"$K2" hostmail --help | grep -q 'cert owner \[--stalwart-acme\]' || fail "hostmail --help must list cert owner"
names_help="$("$K2" hostmail cert names --help)"
printf '%s' "$names_help" | grep -qF 'sudo -n systemctl restart stalwart' \
  || fail "cert names --help: the restart fallback is helper, else sudo -n systemctl"

# Schema.
schema="$("$K2" --schema)" || fail "k2 --schema exited non-zero"
grep -qF '"name": "hostmail cert owner"' <<<"$schema" || fail "schema missing hostmail cert owner"

# Usage errors exit 2 before any request.
expect_usage() {
  set +e
  out="$("$K2" "$@" 2>&1)"
  rc=$?
  set -e
  [ "$rc" -eq 2 ] || fail "'k2 $*' must exit 2, got $rc: $out"
}
expect_usage hostmail cert owner --k2
expect_usage hostmail cert owner extra

WORK="$(mktemp -d -t k2-hostmail-cert-owner-XXXXXX)"
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
SHOW = {"ok": True, "hostname": "mail.example.com", "owner": "stalwart-acme",
        "attached": False, "stalwartAcme": {"type": "Manual"},
        "restoreCommand": "k2 hostmail cert owner --stalwart-acme"}
POST = {"ok": True, "hostname": "mail.example.com", "owner": "stalwart-acme",
        "result": "switched",
        "hint": "Stalwart ACME is on again for mail.example.com (mail host only); "
                "Stalwart queued one ACME order for it"}
STATUS = {"ok": True, "consistent": True, "supported": True, "state": "running",
          "hostname": "mail.example.com",
          "cert": {"host": "mail.example.com", "names": ["mail.example.com"],
                   "state": "issued", "selfSigned": False, "expiresAt": 1900000000,
                   "owner": "k2"}}
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
        self._reply("-", STATUS if path == "/cli/mail/status" else SHOW)
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

show="$(k2 hostmail cert owner)"
printf '%s' "$show" | grep -q '^owner    : stalwart-acme' || fail "show output: $show"
printf '%s' "$show" | grep -q '^stalwart : ACME Manual' || fail "show must print Stalwart's setting: $show"
printf '%s' "$show" | grep -qF 'k2 hostmail cert owner --stalwart-acme' \
  || fail "detached + Manual must name the owner command: $show"

flip="$(k2 hostmail cert owner --stalwart-acme)"
printf '%s' "$flip" | grep -q 'mail host only' || fail "flip output: $flip"
k2 hostmail cert owner --json >/dev/null

status_out="$(k2 hostmail status || true)"
printf '%s' "$status_out" | grep -q '^owner    : k2 — K2 issues, installs and renews it; Stalwart ACME is Manual' \
  || fail "status must print cert.owner: $status_out"

cat >"$WORK/check.py" <<'PY'
import json, sys
rows = [l.split(" ", 2) for l in open(sys.argv[1]).read().splitlines()]
want = [
    ("GET", "/cli/mail/cert/owner", None),
    ("POST", "/cli/mail/cert/owner", {"action": "stalwart-acme"}),
    ("GET", "/cli/mail/cert/owner", None),
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

echo "OK: hostmail cert owner CLI wiring"
