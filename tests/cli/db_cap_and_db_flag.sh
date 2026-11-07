#!/usr/bin/env bash
# 0.46.10: `k2 db cap [<n>|default]` (show or, as the owner, set a
# workspace's ACTIVE-database cap; default 25, 0 = unlimited) and `--db`
# on dsn / migrate / dump / restore / drop. A local fake daemon stands in:
# it logs each request (method, path, body) and answers like the real
# routes do for the owner token and refuses any other token the way
# POST /cli/workspace/set refuses an agent's passport. No real daemon.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CLI="$ROOT/cli/k2"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-db-cap-XXXXXX)"
FAKE_PID=""
cleanup() {
    if [ -n "$FAKE_PID" ]; then kill "$FAKE_PID" 2>/dev/null || true; wait "$FAKE_PID" 2>/dev/null || true; fi
    rm -rf "$FAKE_DIR"
    hermetic_cli_cleanup
}
trap cleanup EXIT

cat >"$FAKE_DIR/fake.py" <<'PY'
import http.server, json, sys, urllib.parse
log_path, port_path = sys.argv[1], sys.argv[2]
OWNER = "owner-token"
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        u = urllib.parse.urlsplit(self.path)
        q = dict(urllib.parse.parse_qsl(u.query))
        with open(log_path, "a") as f:
            shown = {k: v for k, v in q.items() if k != "token"}
            f.write("%s %s %s %s\n" % (self.command, u.path, json.dumps(shown, sort_keys=True), body))
        status, out = 200, {"ok": True}
        if u.path == "/cli/workspace/set":
            if q.get("token") != OWNER:
                status, out = 403, {"error": "invalid or missing token"}
            else:
                out = {"ok": True, "changed": True}
        elif u.path == "/cli/db/list":
            out = {"ok": True, "databases": [], "cap": 25}
        elif u.path == "/cli/db/dsn":
            out = {"ok": True, "name": q.get("db", "ws_x"), "dsn": "postgresql://a@h:1/" + q.get("db", "ws_x")}
        raw = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
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
[ -s "$FAKE_DIR/port" ] || fail "fake daemon did not start"
export K2_PORT="$(cat "$FAKE_DIR/port")"
WS="$FAKE_DIR/ws"
mkdir -p "$WS"

run() {
    set +e
    OUT="$("$CLI" "$@" 2>&1)"
    RC=$?
    set -e
}

# Help names the verb, the default and who may set it.
help="$("$CLI" db --help)"
echo "$help" | grep -q 'cap \[<n>|default\]' || fail "k2 db --help must list cap"
echo "$help" | grep -q 'default 25' || fail "k2 db --help must say the default is 25"
echo "$help" | grep -q 'only the owner' || fail "k2 db --help must say only the owner sets the cap"
echo "$help" | grep -q 'dsn --db <name|id>' || fail "k2 db --help must list dsn --db"
study="$("$CLI" study db)"
echo "$study" | grep -q 'up to 25 ACTIVE databases' || fail "k2 study db must give the cap"
echo "$study" | grep -q 'cap=1' && fail "k2 study db still says cap=1"

# Usage errors exit 2 before any request.
: >"$FAKE_DIR/requests.log"
run db cap -3 --workspace "$WS"
[ "$RC" -eq 2 ] || fail "k2 db cap -3 must exit 2, got $RC: $OUT"
run db cap many --workspace "$WS"
[ "$RC" -eq 2 ] || fail "k2 db cap many must exit 2, got $RC: $OUT"
run db cap 1 2 --workspace "$WS"
[ "$RC" -eq 2 ] || fail "k2 db cap 1 2 must exit 2, got $RC: $OUT"
run db dsn --db
[ "$RC" -eq 2 ] || fail "k2 db dsn --db (no value) must exit 2, got $RC: $OUT"
[ ! -s "$FAKE_DIR/requests.log" ] || fail "usage errors sent requests: $(cat "$FAKE_DIR/requests.log")"

# An agent's passport: refused, exit 3, says to ask the owner.
export K2_HOOK_TOKEN=agent-passport
: >"$FAKE_DIR/requests.log"
run db cap 30 --workspace "$WS"
[ "$RC" -eq 3 ] || fail "an agent setting the cap must exit 3, got $RC: $OUT"
echo "$OUT" | grep -q 'owner_only' || fail "agent refusal must be owner_only: $OUT"
echo "$OUT" | grep -q 'ask your human' || fail "agent refusal must say to ask the human: $OUT"

# Reading is anyone's.
run db cap --workspace "$WS"
[ "$RC" -eq 0 ] || fail "reading the cap must work for an agent, got $RC: $OUT"
echo "$OUT" | grep -q 'database cap: 25' || fail "read must print the cap: $OUT"

# The owner sets a number, 0 (unlimited) and default.
export K2_HOOK_TOKEN=owner-token
: >"$FAKE_DIR/requests.log"
run db cap 40 --workspace "$WS"
[ "$RC" -eq 0 ] || fail "owner set must exit 0, got $RC: $OUT"
echo "$OUT" | grep -q 'database cap: 40' || fail "owner set must print the cap: $OUT"
grep -q "^POST /cli/workspace/set .*\"db_active_cap\": \"40\"" "$FAKE_DIR/requests.log" \
    || fail "owner set must POST db_active_cap=40: $(cat "$FAKE_DIR/requests.log")"
grep -qF "\"project\": \"$WS\"" "$FAKE_DIR/requests.log" || fail "owner set must name the workspace: $(cat "$FAKE_DIR/requests.log")"
run db cap 0 --workspace "$WS" --json
[ "$RC" -eq 0 ] || fail "owner set 0 must exit 0, got $RC: $OUT"
echo "$OUT" | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v["cap"]==0 and v["unlimited"] is True, v' \
    || fail "cap 0 --json must say unlimited: $OUT"
run db cap default --workspace "$WS"
[ "$RC" -eq 0 ] || fail "owner set default must exit 0, got $RC: $OUT"
echo "$OUT" | grep -q '25 (the default)' || fail "default must print 25 (the default): $OUT"
grep -q '"db_active_cap": "default"' "$FAKE_DIR/requests.log" || fail "default must POST 'default': $(cat "$FAKE_DIR/requests.log")"

# --db travels on dsn (query) and migrate / dump / restore / drop (body).
: >"$FAKE_DIR/requests.log"
run db dsn --db ws_x_two --workspace "$WS"
[ "$RC" -eq 0 ] || fail "dsn --db must exit 0, got $RC: $OUT"
grep -q '^GET /cli/db/dsn .*"db": "ws_x_two"' "$FAKE_DIR/requests.log" || fail "dsn --db not sent: $(cat "$FAKE_DIR/requests.log")"
run db migrate --db ws_x_two --workspace "$WS"
grep -q '^POST /cli/db/migrate .*"db": *"ws_x_two"' "$FAKE_DIR/requests.log" || fail "migrate --db not sent: $(cat "$FAKE_DIR/requests.log")"
run db dump --db=ws_x_two --workspace "$WS"
grep -q '^POST /cli/db/dump .*"db": *"ws_x_two"' "$FAKE_DIR/requests.log" || fail "dump --db not sent: $(cat "$FAKE_DIR/requests.log")"
run db restore .k2/db/dumps/a.dump --db ws_x_two --workspace "$WS"
grep -q '^POST /cli/db/restore .*"file": *".k2/db/dumps/a.dump".*"db": *"ws_x_two"' "$FAKE_DIR/requests.log" \
    || fail "restore --db not sent: $(cat "$FAKE_DIR/requests.log")"
run db drop --yes --db ws_x_two --workspace "$WS"
grep -q '^POST /cli/db/drop .*"db": *"ws_x_two"' "$FAKE_DIR/requests.log" || fail "drop --db not sent: $(cat "$FAKE_DIR/requests.log")"

echo "PASS: k2 db cap and --db"
