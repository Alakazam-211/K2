#!/usr/bin/env bash
# 0.46.17: the CLI half of a test login's rooms.
#   - `k2 app user add <app> <login> --test --role R` from an agent ends with
#     the Ask with: line when the role names rooms past the agent's own;
#   - `k2 app user rooms-ok <app> <login> [--assign <login>]` posts the App
#     route (with ?assign=), says where the card is and exits 2 while it
#     waits; an owner's answer (opened) exits 0; help sends nothing.
# A local fake daemon stands in and logs each request. No real daemon, no ~/.k2.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; CLI="$CLI_UNDER_TEST"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-tg-rooms-XXXXXX)"
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
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        u = urllib.parse.urlsplit(self.path)
        q = dict(urllib.parse.parse_qsl(u.query))
        with open(log_path, "a") as f:
            f.write("%s %s %s %s\n" % (self.command, u.path, json.dumps({k: v for k, v in q.items() if k != "token"}, sort_keys=True), body))
        b = json.loads(body or "{}") if body.startswith("{") else {}
        status, out = 200, {"ok": True}
        if u.path == "/cli/app/test-guest/add":
            out = {"ok": True, "username": b.get("username"), "app": "qa1", "role": "tester", "rooms": [], "expiresAt": 4102444800,
                   "tied": True, "note": "a test guest",
                   "roomsWait": {"rooms": ["sales"], "why": "a test login your agent made signs in only to ops's own room",
                                 "ask": "k2 app user rooms-ok qa1 %s" % b.get("username")}}
        elif u.path == "/cli/app/test-guest/rooms-ok":
            if b.get("username") == "owned":
                out = {"ok": True, "status": "opened", "username": "owned", "rooms": ["sales"], "note": "owned can now use sales"}
            else:
                out = {"ok": True, "status": "asked", "username": b.get("username"), "role": "tester", "rooms": ["sales"],
                       "card": {"id": "pr_1", "card": "c1", "thread": "ops", "routedTo": ["ada"]},
                       "where": "a permission card pr_1 is on ops's Thread for ada"}
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
export K2_HOOK_TOKEN=owner-token

run() {
    set +e
    OUT="$("$CLI" "$@" 2>"$FAKE_DIR/stderr")"
    RC=$?
    ERR="$(cat "$FAKE_DIR/stderr")"
    set -e
}

: >"$FAKE_DIR/requests.log"
run app user rooms-ok --help
[ "$RC" -eq 0 ] || fail "help exits 0"
grep -qF 'k2 app user rooms-ok <app-id> <test-login>' <<<"$OUT" || fail "help names the verb: $OUT"
if grep -q '^POST' "$FAKE_DIR/requests.log"; then fail "help acted: $(cat "$FAKE_DIR/requests.log")"; fi
app_help="$("$CLI" app --help 2>&1)"
grep -qF 'k2 app user rooms-ok <app-id> <test-login>' <<<"$app_help" || fail "k2 app --help names rooms-ok"
grep -qF "A TEST LOGIN'S ROOMS (0.46.17)" <<<"$("$CLI" study apps)" || fail "k2 study apps explains it"

run app user add qa1 tess --test --role tester --expires 7d
[ "$RC" -eq 0 ] || fail "add exits 0: $RC $ERR"
grep -qF 'Ask with: k2 app user rooms-ok qa1 tess' <<<"$OUT" || fail "the add ends with its Ask with: line: $OUT"
grep -qF 'Its role also names sales' <<<"$OUT" || fail "and says which rooms wait: $OUT"

: >"$FAKE_DIR/requests.log"
run app user rooms-ok qa1 tess --assign ada
[ "$RC" -eq 2 ] || fail "asked exits 2 while it waits, got $RC: $ERR"
grep -qF 'POST /cli/app/test-guest/rooms-ok {"assign": "ada"}' "$FAKE_DIR/requests.log" || fail "posts the route with ?assign=: $(cat "$FAKE_DIR/requests.log")"
grep -qF '"username": "tess"' "$FAKE_DIR/requests.log" || fail "names the login: $(cat "$FAKE_DIR/requests.log")"
grep -qF 'Asked: tess into sales with its role tester.' <<<"$OUT" || fail "says it asked: $OUT"
grep -qF 'permission card pr_1' <<<"$OUT" || fail "says where the card is: $OUT"

run app user rooms-ok qa1 owned
[ "$RC" -eq 0 ] || fail "opened exits 0, got $RC"
run app user rooms-ok qa1
[ "$RC" -eq 2 ] || fail "a missing login is a usage error, got $RC"

echo "PASS: app test guest rooms CLI"
