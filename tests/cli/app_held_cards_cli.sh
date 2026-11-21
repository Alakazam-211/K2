#!/usr/bin/env bash
# 0.46.17: the CLI half of held App requests as permission cards.
#   - `--assign <login>[,<login>…]` anywhere before `--` on a k2 app command
#     rides its POST as ?assign= (never into the subcommand's own flags);
#     a bare --assign is a usage error that sends nothing; GETs don't carry it.
#   - a held answer prints where its card is and who it went to;
#   - `k2 app approvals <id> --state S` asks for that request / state and
#     prints each request's card line.
#   - `k2 publish run --assign` sends it in the body.
# A local fake daemon stands in and logs each request (method, path, query
# without the token, body). No real daemon, no ~/.k2.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; CLI="$CLI_UNDER_TEST"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-held-cards-XXXXXX)"
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
WHERE = "a permission card w_abc123 is on a's Thread: sent to ada, as you asked (--assign). They approve it there, or in Settings → App Approvals"
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
        if u.path == "/cli/app/db/connect":
            status, out = 202, {"ok": True, "status": "held", "request": {"id": "w_abc123", "summary": "k2 app db connect app1 → a new database"},
                                "switches": [], "diff": {"lines": ["connect App app1"]}, "hint": "one card for everything connecting needs",
                                "card": {"raised": True, "new": True, "id": "w_abc123", "where": WHERE}}
        elif u.path == "/cli/app/egress/add":
            status, out = 202, {"ok": True, "status": "held", "request": {"id": "w_eg1", "summary": "egress api.example.com for app1"},
                                "hint": "held for the owner", "card": {"raised": True, "where": "a permission card w_eg1 is on a's Thread: sent to every owner and admin (ada, bo); pass --assign <login> to send it to one."}}
        elif u.path == "/cli/app/approvals":
            card = {"id": "w_abc123", "state": "pending", "card": "c1", "thread": "a", "routedTo": [{"login": "ada", "role": "admin"}], "narrowed": True}
            out = {"ok": True, "pendingCount": 1, "canApprove": False, "requests": [
                {"id": "w_abc123", "status": "pending", "kind": "manage", "appLabel": "app1", "workspaceHandle": "a",
                 "requestedBy": "agent:a", "summary": "k2 app db connect app1 → a new database", "reasons": [], "card": card},
                {"id": "w_old", "status": "denied", "kind": "manage", "appLabel": "app1", "workspaceHandle": "a",
                 "requestedBy": "agent:a", "summary": "older", "reasons": [], "decidedBy": "owner", "card": None}]}
        elif u.path == "/cli/publish/run":
            status, out = 202, {"ok": True, "status": "held", "request": {"id": "w_run1", "summary": "run web"}}
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
reqs_to() { grep -F " $1 " "$FAKE_DIR/requests.log" || true; }

# ── help names it (help never acts) ──────────────────────────────────
: >"$FAKE_DIR/requests.log"
grace_help="$("$CLI" app approvals --help 2>&1)"
grep -qF -- '--assign <login>[,<login>…]' <<<"$grace_help" || fail "k2 app approvals --help names --assign"
grep -qF -- 'k2 app approvals [<request-id>] [--app A] [--state S|--all]' <<<"$grace_help" || fail "approvals help names <request-id> and --state"
pub_help="$("$CLI" publish run --help 2>&1)"
grep -qF -- '--assign <login>[,<login>…]' <<<"$pub_help" || fail "k2 publish run --help names --assign"
study="$("$CLI" study apps)"
grep -qF 'HELD FOR THE OWNER: THE CARD (0.46.17)' <<<"$study" || fail "k2 study apps explains the card"
run app db connect app1 --new --assign ada --help
[ "$RC" -eq 0 ] || fail "--help with --assign prints help, got $RC: $ERR"
if grep -q '^POST' "$FAKE_DIR/requests.log"; then fail "help acted: $(cat "$FAKE_DIR/requests.log")"; fi

# ── --assign rides the POST, not the subcommand's flags ──────────────
run app db connect app1 --new --assign ada
[ "$RC" -eq 2 ] || fail "held exits 2, got $RC: $OUT $ERR"
reqs_to /cli/app/db/connect | grep -qF '{"assign": "ada"}' || fail "connect carries ?assign=ada: $(cat "$FAKE_DIR/requests.log")"
reqs_to /cli/app/db/connect | grep -q '"assign": "ada".*"new": true' || fail "the body is the command's own: $(reqs_to /cli/app/db/connect)"
grep -qF 'owner: a permission card w_abc123' <<<"$OUT" || fail "the held answer says where the card is: $OUT"
grep -qF 'sent to ada, as you asked (--assign)' <<<"$OUT" || fail "and who it went to: $OUT"

# Before the subcommand, a list, the = form.
: >"$FAKE_DIR/requests.log"
run app --assign ada,owner egress add app1 api.example.com
[ "$RC" -eq 2 ] || fail "egress held exits 2, got $RC: $OUT $ERR"
reqs_to /cli/app/egress/add | grep -qF '{"assign": "ada,owner"}' || fail "egress carries the list: $(cat "$FAKE_DIR/requests.log")"
reqs_to /cli/app/egress/add | grep -qF '"hosts": ["api.example.com"]' || fail "--assign is not a host: $(reqs_to /cli/app/egress/add)"
grep -qF 'pass --assign <login> to send it to one' <<<"$OUT" || fail "without --assign the agent is told how: $OUT"
: >"$FAKE_DIR/requests.log"
run app egress add app1 api.example.com --assign=bo
reqs_to /cli/app/egress/add | grep -qF '{"assign": "bo"}' || fail "--assign=bo: $(cat "$FAKE_DIR/requests.log")"

# Without it: no assign on the wire.
: >"$FAKE_DIR/requests.log"
run app egress add app1 api.example.com
reqs_to /cli/app/egress/add | grep -qF ' {} ' || fail "no --assign, no assign: $(cat "$FAKE_DIR/requests.log")"

# A bare --assign: usage, nothing sent.
: >"$FAKE_DIR/requests.log"
run app egress add app1 api.example.com --assign
[ "$RC" -eq 2 ] || fail "a bare --assign is a usage error (exit 2), got $RC"
grep -qF '"error":"usage"' <<<"$ERR" || fail "says usage: $ERR"
[ ! -s "$FAKE_DIR/requests.log" ] || fail "a usage error sent requests: $(cat "$FAKE_DIR/requests.log")"

# After --, it's the command's own (k2 app exec): left alone.
: >"$FAKE_DIR/requests.log"
run app exec app1 -- echo --assign ada
if grep -qF '{"assign": "ada"}' "$FAKE_DIR/requests.log"; then fail "--assign after -- was taken: $(cat "$FAKE_DIR/requests.log")"; fi
grep -qF -- '"--assign", "ada"' "$FAKE_DIR/requests.log" || fail "after --, the command keeps it: $(cat "$FAKE_DIR/requests.log")"

# ── k2 app approvals: one by id, by state, with the card line ────────
: >"$FAKE_DIR/requests.log"
run app approvals w_abc123
[ "$RC" -eq 0 ] || fail "approvals <id> exits 0, got $RC: $ERR"
reqs_to /cli/app/approvals | grep -qF '"request": "w_abc123"' || fail "asks for that request: $(cat "$FAKE_DIR/requests.log")"
grep -qF 'card w_abc123: pending, on the Thread of a, sent to ada' <<<"$OUT" || fail "the card line: $OUT"
: >"$FAKE_DIR/requests.log"
run app approvals --state denied
reqs_to /cli/app/approvals | grep -qF '"state": "denied"' || fail "--state denied: $(cat "$FAKE_DIR/requests.log")"
run app approvals --state nonsense
[ "$RC" -eq 2 ] || fail "a bad --state is a usage error, got $RC"
run app approvals w_a w_b
[ "$RC" -eq 2 ] || fail "two ids is a usage error, got $RC"

# ── k2 publish run --assign: in the body ─────────────────────────────
: >"$FAKE_DIR/requests.log"
run publish run web --cmd "python3 -m http.server 8999" --port 8999 --no-tunnel --assign ada
reqs_to /cli/publish/run | grep -qF '"assign": "ada"' || fail "publish run sends assign: $(cat "$FAKE_DIR/requests.log")"

echo "PASS: app held cards CLI"
