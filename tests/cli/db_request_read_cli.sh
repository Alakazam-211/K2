#!/usr/bin/env bash
# 0.46.17: the CLI half of reading another workspace's database on a card:
# `k2 db browse` (and `k2 db list --all`), `k2 db request-read`, `k2 db
# requests` and `k2 app db connect <app> --read-only`. Exit 0 done or the card
# is up (2 for connect --read-only, as connect's held), 2 usage, 3 refused, 4
# not found; help never sends a request; no DSN password is ever printed. A
# local fake daemon stands in and logs each request (method, path, query
# without the token, body). No real daemon, no ~/.k2.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; CLI="$CLI_UNDER_TEST"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-db-read-XXXXXX)"
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
DBS = [{"id": "d1", "name": "ws_o_jobs", "owner": "ops", "apps": ["jobs"], "yours": None},
       {"id": "d2", "name": "ws_o_notes", "owner": "ops", "apps": [], "yours": "read"}]
class H(http.server.BaseHTTPRequestHandler):
    def _any(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode() if n else ""
        u = urllib.parse.urlsplit(self.path)
        q = dict(urllib.parse.parse_qsl(u.query))
        with open(log_path, "a") as f:
            shown = {k: v for k, v in q.items() if k != "token"}
            f.write("%s %s %s %s\n" % (self.command, u.path, json.dumps(shown, sort_keys=True), body))
        b = json.loads(body or "{}") if body.startswith("{") else {}
        status, out = 200, {"ok": True}
        if u.path == "/cli/db/browse":
            rows = [d for d in DBS if not q.get("app") or q["app"] in d["apps"]]
            if q.get("app") == "nope":
                status, out = 404, {"ok": False, "error": {"code": "app_not_found", "hint": "this server has no App nope"}}
            else:
                out = {"ok": True, "databases": rows, "hint": "names only: no DSN"}
        elif u.path == "/cli/db/request-read":
            target = b.get("db") or b.get("app")
            if target == "missing":
                status, out = 404, {"ok": False, "error": {"code": "not_found", "hint": "this server has no database missing"}}
            elif target == "mine":
                status, out = 409, {"ok": False, "error": {"code": "db_read_own", "hint": "you own it"}}
            elif target == "off":
                status, out = 403, {"ok": False, "error": {"code": "permission_cards_off", "hint": "an owner or admin turns on \"Let this agent ask for permission\""}}
            elif b.get("connect") and target == "readable":
                out = {"ok": True, "status": "read_only", "readOnly": True, "name": "ws_o_notes",
                       "dsn": "postgres://ws_r_agent@127.0.0.1:5432/ws_o_notes", "dsnHasPassword": False, "dsnHint": "no password shown"}
            elif b.get("dryRun"):
                out = {"status": "dry_run", "action": "db.read", "text": "Let reader read the ws_o_jobs database (owned by ops, App jobs).",
                       "approvers": "owner or admin", "routedTo": [{"login": "ada", "role": "admin"}]}
            else:
                out = {"id": "pr_1", "status": "pending", "action": "db.read", "text": "Let reader read the ws_o_jobs database (owned by ops, App jobs).",
                       "approvers": "owner or admin", "routedTo": [{"login": b.get("assign") or "rosson", "role": "owner"}], "card": "c1", "addr": "reader",
                       "db": "ws_o_jobs", "dbId": "d1", "owner": "ops, App jobs", "next": "check once: k2 db requests"}
        elif u.path == "/cli/permission/status":
            out = {"requests": [{"id": "pr_1", "action": "db.read", "params": {"db": "ws_o_jobs"}, "state": "approved",
                                 "message": "reader can read ws_o_jobs now"}]}
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
last_req() { tail -n 1 "$FAKE_DIR/requests.log"; }

# ── help never acts ──────────────────────────────────────────────────
: >"$FAKE_DIR/requests.log"
help="$("$CLI" db --help)"
for want in "browse [--app <app>]" "request-read --db <name|id> | --app <app>" "--assign <login>" "requests [--json]" "list --all" "permission_cards_off"; do
    grep -qF -- "$want" <<<"$help" || fail "k2 db --help names $want"
done
for args in "db request-read --help" "db request-read --db x -h" "db browse --help" "db requests --help" "app db connect jobs --read-only --help"; do
    # shellcheck disable=SC2086
    run $args
    [ "$RC" -eq 0 ] || fail "k2 $args prints help, exit 0, got $RC: $ERR"
done
app_help="$("$CLI" app db --help)"
grep -qF -- "--read-only [--db <name>] [--why" <<<"$app_help" || fail "k2 app db --help names --read-only"
[ ! -s "$FAKE_DIR/requests.log" ] || fail "help sent requests: $(cat "$FAKE_DIR/requests.log")"

# ── usage errors send nothing ───────────────────────────────────────
for args in "db request-read" "db request-read --db" "db request-read --why" "db request-read --db x --frob" "db browse --app" "db requests extra" \
            "app db connect jobs --read-only --new" "app db connect jobs --read-only --as X_URL" "app db connect jobs --new --assign ada"; do
    # shellcheck disable=SC2086
    run $args
    case "$args" in app*) want=1 ;; *) want=2 ;; esac
    [ "$RC" -eq "$want" ] || fail "k2 $args is a usage error (exit $want), got $RC: $ERR"
    grep -q 'usage' <<<"$ERR" || fail "k2 $args says usage: $ERR"
done
[ ! -s "$FAKE_DIR/requests.log" ] || fail "usage errors sent requests: $(cat "$FAKE_DIR/requests.log")"

# ── browse ───────────────────────────────────────────────────────────
run db browse
[ "$RC" -eq 0 ] || fail "browse exits 0, got $RC: $ERR"
grep -q '^2 database(s) on this server' <<<"$OUT" || fail "browse counts: $OUT"
grep -qE 'ws_o_jobs +owner ops +apps jobs +you -' <<<"$OUT" || fail "browse row: $OUT"
grep -qE 'ws_o_notes .* you read' <<<"$OUT" || fail "browse says what you hold: $OUT"
last_req | grep -q '^GET /cli/db/browse ' || fail "browse is a GET: $(last_req)"
run db list --all
[ "$RC" -eq 0 ] && grep -q 'database(s) on this server' <<<"$OUT" || fail "list --all is browse: $RC $OUT"
run db browse --app jobs --json
[ "$RC" -eq 0 ] || fail "browse --app exits 0"
last_req | grep -q '"app": "jobs"' || fail "browse --app sends app=: $(last_req)"
python3 -c 'import json,sys; v=json.loads(sys.argv[1]); assert [d["name"] for d in v["databases"]] == ["ws_o_jobs"], v' "$OUT" || fail "--json: $OUT"
run db browse --app nope
[ "$RC" -eq 4 ] && grep -q 'app_not_found' <<<"$ERR" || fail "an unknown App is not found (exit 4): $RC $ERR"

# ── request-read ─────────────────────────────────────────────────────
run db request-read --db ws_o_jobs --why "verify the import" --assign ada
[ "$RC" -eq 0 ] || fail "a card up exits 0, got $RC: $OUT $ERR"
last_req | grep -q '^POST /cli/db/request-read ' || fail "request-read is a POST: $(last_req)"
last_req | grep -q '"db": "ws_o_jobs", "why": "verify the import", "assign": "ada"' || fail "the body: $(last_req)"
last_req | grep -q 'dryRun' && fail "no dryRun unless asked: $(last_req)"
grep -q '^Card pr_1 raised on your Thread (reader)' <<<"$OUT" || fail "card line: $OUT"
grep -qF 'Card: Let reader read the ws_o_jobs database (owned by ops, App jobs).' <<<"$OUT" || fail "the card's words: $OUT"
grep -q '^Asks: ada' <<<"$OUT" || fail "who it asks: $OUT"
grep -q '^Next: check once: k2 db requests' <<<"$OUT" || fail "next: $OUT"
run db request-read --app jobs --dry-run
[ "$RC" -eq 0 ] && grep -q '^Would ask: Let reader read' <<<"$OUT" || fail "dry run: $RC $OUT"
last_req | grep -q '"app": "jobs", "dryRun": true' || fail "dry run body: $(last_req)"
run db request-read --db missing
[ "$RC" -eq 4 ] && grep -q '"code": "not_found"' <<<"$ERR" || fail "not found exits 4: $RC $ERR"
run db request-read --db mine
[ "$RC" -eq 3 ] && grep -q 'db_read_own' <<<"$ERR" || fail "your own is refused, exit 3: $RC $ERR"
run db request-read --db off
[ "$RC" -eq 3 ] && grep -q 'permission_cards_off' <<<"$ERR" && grep -q 'Let this agent ask for permission' <<<"$ERR" || fail "cards off, exit 3: $RC $ERR"

# ── requests ─────────────────────────────────────────────────────────
run db requests
[ "$RC" -eq 0 ] || fail "requests exits 0"
last_req | grep -q '^GET /cli/permission/status {"action": "db.read"}' || fail "requests reads db.read asks: $(last_req)"
grep -qE 'pr_1 +approved +ws_o_jobs +reader can read' <<<"$OUT" || fail "requests row: $OUT"

# ── app db connect --read-only ───────────────────────────────────────
run app db connect jobs --read-only --why "verify"
[ "$RC" -eq 2 ] || fail "connect --read-only with the card up exits 2 (held), got $RC: $OUT $ERR"
last_req | grep -q '^POST /cli/db/request-read .*"app": "jobs", "why": "verify", "connect": true' || fail "the same ask, with connect: $(last_req)"
grep -q '^Card pr_1 raised' <<<"$OUT" || fail "card line: $OUT"
run app db connect readable --read-only
[ "$RC" -eq 0 ] || fail "a database you read answers its DSN, exit 0, got $RC: $ERR"
[ "$OUT" = "postgres://ws_r_agent@127.0.0.1:5432/ws_o_notes" ] || fail "the read-only DSN, no password: $OUT"
grep -q '^read-only; no password shown' <<<"$ERR" || fail "says it is read-only: $ERR"
run app db connect mine --read-only
[ "$RC" -eq 3 ] && grep -q 'db_read_own' <<<"$ERR" || fail "your own App's database: db_read_own, exit 3: $RC $ERR"

echo "PASS: db_request_read_cli"
