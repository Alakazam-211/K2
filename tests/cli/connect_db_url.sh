#!/usr/bin/env bash
# 0.46.6: `k2 connect` sends its k2.dev account calls to the account server
# K2_CONNECT_DB_URL or the `connectDbUrl` setting names; the default is
# unchanged; no automatic fallback between hosts; the anon key never shows
# in output. A local fake account server stands in for the real one: this
# test never reaches Supabase or db.k2.dev (the default-path case runs
# behind a dead proxy).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CLI="$ROOT/cli/k2"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env
unset K2_AIRGAP K2_CONNECT_DB_URL
# A box that ran a daemon has ~/.k2 (the CLI saves the session there).
mkdir -p "$HOME/.k2"

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-connect-db-XXXXXX)"
FAKE_PID=""
cleanup() {
    if [ -n "$FAKE_PID" ]; then kill "$FAKE_PID" 2>/dev/null || true; wait "$FAKE_PID" 2>/dev/null || true; fi
    rm -rf "$FAKE_DIR"
    hermetic_cli_cleanup
}
trap cleanup EXIT

# The fake account server: logs "METHOD PATH UA=<ua> APIKEY=<set|missing>"
# per request (never the key itself) and answers 401 to everything.
cat >"$FAKE_DIR/fake.py" <<'PY'
import http.server, sys
log_path, port_path = sys.argv[1], sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        if n:
            self.rfile.read(n)
        with open(log_path, "a") as f:
            f.write("%s %s UA=%s APIKEY=%s\n" % (
                self.command, self.path, self.headers.get("User-Agent", ""),
                "set" if self.headers.get("apikey") else "missing"))
        body = b'{"message":"JWT expired"}'
        self.send_response(401)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_GET = do_POST = _any
    def log_message(self, *a):
        pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
open(port_path, "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 -I "$FAKE_DIR/fake.py" "$FAKE_DIR/requests.log" "$FAKE_DIR/port" &
FAKE_PID=$!
for _ in $(seq 1 50); do [ -s "$FAKE_DIR/port" ] && break; sleep 0.1; done
[ -s "$FAKE_DIR/port" ] || fail "fake account server did not start"
FAKE="http://127.0.0.1:$(cat "$FAKE_DIR/port")"

ANON_PREFIX="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIs"

login() {
    # --token: no password prompt; the first account call lists subdomains.
    set +e
    OUT="$("$CLI" connect login --token test-access-token --subdomain rosson 2>&1)"
    RC=$?
    set -e
    case "$OUT" in *"$ANON_PREFIX"*) fail "the anon key appeared in output: $OUT" ;; esac
}

# 1. K2_CONNECT_DB_URL wins; a 401 is reported once, never retried elsewhere.
: >"$FAKE_DIR/requests.log"
K2_CONNECT_DB_URL="$FAKE/" login
[ "$RC" -ne 0 ] || fail "env override: a 401 must fail the login. output: $OUT"
echo "$OUT" | grep -qF "account server $FAKE (from K2_CONNECT_DB_URL)" || fail "env override: no note. output: $OUT"
[ "$(wc -l <"$FAKE_DIR/requests.log" | tr -d ' ')" = 1 ] || fail "env override: expected 1 request: $(cat "$FAKE_DIR/requests.log")"
grep -q '^GET /rest/v1/subdomains?select=label' "$FAKE_DIR/requests.log" || fail "env override: wrong request: $(cat "$FAKE_DIR/requests.log")"
grep -q 'UA=k2-cli/' "$FAKE_DIR/requests.log" || fail "env override: no k2-cli User-Agent: $(cat "$FAKE_DIR/requests.log")"
grep -q 'APIKEY=set' "$FAKE_DIR/requests.log" || fail "env override: apikey header missing"
echo "ok K2_CONNECT_DB_URL override, one request, no fallback on 401"

# 2. The connectDbUrl setting (same settings.json the daemon reads).
printf '{"connectDbUrl": "%s"}\n' "$FAKE" >"$HOME/.k2/settings.json"
: >"$FAKE_DIR/requests.log"
login
[ "$RC" -ne 0 ] || fail "setting: a 401 must fail the login"
echo "$OUT" | grep -qF "account server $FAKE (from the connectDbUrl setting)" || fail "setting: no note. output: $OUT"
grep -q '^GET /rest/v1/subdomains' "$FAKE_DIR/requests.log" || fail "setting: the fake got no request"
echo "ok connectDbUrl setting"

# 3. An invalid env value is skipped with a warning; the setting still wins.
: >"$FAKE_DIR/requests.log"
K2_CONNECT_DB_URL="http://evil.example.test" login
echo "$OUT" | grep -q "K2_CONNECT_DB_URL ignored: must be an https:// URL" || fail "invalid env: no warning. output: $OUT"
echo "$OUT" | grep -qF "(from the connectDbUrl setting)" || fail "invalid env: setting not used. output: $OUT"
grep -q '^GET /rest/v1/subdomains' "$FAKE_DIR/requests.log" || fail "invalid env: the fake got no request"
echo "ok invalid env skipped"

# 4. Default: no override → no note, the fake is never called. A dead proxy
# keeps the request off the real network; the login must fail to connect.
rm -f "$HOME/.k2/settings.json"
: >"$FAKE_DIR/requests.log"
https_proxy="http://127.0.0.1:1" HTTPS_PROXY="http://127.0.0.1:1" login
[ "$RC" -ne 0 ] || fail "default: expected a connect failure behind the dead proxy. output: $OUT"
case "$OUT" in *"account server"*) fail "default: must not print an override note. output: $OUT" ;; esac
[ ! -s "$FAKE_DIR/requests.log" ] || fail "default: the fake was called: $(cat "$FAKE_DIR/requests.log")"
echo "ok default unchanged"

echo "connect_db_url CLI tests passed"
