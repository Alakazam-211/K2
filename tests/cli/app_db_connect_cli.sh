#!/usr/bin/env bash
# 0.46.11 (prd-app-database-integrations-v1 S2): the CLI half of `k2 app db
# connect|status|check|disconnect`: exit 0 PASS, 1 FAIL or usage (R7), 2 held
# (one card, its Ask with: line), 3 refused (its fix line); never a URL
# printed. A local fake daemon stands in and logs each request (method,
# path, query without the token, body). No real daemon, no ~/.k2.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CLI="$ROOT/cli/k2"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-app-db-XXXXXX)"
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
FULL = "postgres://ws_a_agent:S3CRETpw0123456789@127.0.0.1:5432/ws_a"
SHORT = "postgres://ws_a_agent@127.0.0.1:5432/ws_a"
PERM = {"action": "db.agent-access", "command": "k2 permission request db.agent-access level=write --why \"<what you need it for>\""}
def connected(ok=True, step=None):
    c = {"ok": ok, "version": "PostgreSQL 17.6" if ok else None, "latencyMs": 3, "from": "app", "at": 1}
    if not ok:
        c.update({"step": step, "error": "28000 K2 database door: no [k2:db_door_role_refused]", "hint": "k2 app doctor feedback",
                  "appOutput": ["Error: connect ECONNREFUSED", "exit 1"]})
    return {"ok": ok, "app": "feedback", "as": "DATABASE_URL", "kind": "container", "db": "ws_a_app_feedback", "cluster": "17/main",
            "major": 17, "roles": ["ws_a_app_feedback_app_feedback__g1", "ws_a_app_feedback_migrator"],
            "secrets": ["app-db.feedback.DATABASE_URL", "app-db.feedback.DATABASE_MIGRATE_URL"], "doorOpen": True,
            "check": c, "created": True, "rehost": {"result": "done", "deployment": "d-2"}}
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
        if u.path == "/cli/db/dsn":
            if q.get("ref") == "1":
                out = {"ok": True, "ref": "k2v://ws/a/" + (q.get("as") or ("DATABASE_URL_" + q.get("db", "").upper() if q.get("db") else "DATABASE_URL")),
                       "name": q.get("as") or "x"}
            elif q.get("reveal") == "1":
                out = {"ok": True, "name": "ws_a", "dsn": FULL, "dsnHasPassword": True}
            else:
                out = {"ok": True, "name": "ws_a", "dsn": SHORT, "dsnHasPassword": False, "dsnHint": "no password shown: k2 db run -- <cmd>"}
        elif u.path == "/cli/db/create":
            status, out = 403, {"ok": False, "error": {"code": "forbidden", "hint": "workspace db_agent_access is 'off'"}, "permission": PERM}
        elif u.path == "/cli/db/grant":
            status, out = 403, {"ok": False, "error": {"code": "forbidden", "hint": "owner only"},
                                "permission": {"requestable": False, "where": "a person does this"}}
        elif u.path == "/cli/app/db/connect":
            app = b.get("app")
            if app == "held":
                status, out = 202, {"ok": True, "status": "held", "request": {"id": "w_abc123", "summary": "k2 app db connect held → a new database"},
                                    "switches": [{"id": "apps_build", "name": "Build and run Apps", "where": "Settings → Workspaces → a → Agent → Apps", "on": False},
                                                 {"id": "app_core_db", "name": "Database (what its Apps may use)", "where": "Settings → …", "on": True}],
                                    "diff": {"lines": ["connect App held"]}, "hint": "one card: k2 app approve w_abc123",
                                    "permission": {"action": "apps.approve", "command": "k2 permission request apps.approve request=w_abc123 --why \"<what you need it for>\""}}
            elif app == "nopg":
                status, out = 409, {"error": {"code": "app_db_cluster_missing", "hint": "Postgres 16 isn't running on this box yet"}}
            elif app == "bad":
                status, out = 400, {"error": {"code": "usage", "hint": "--new or --db, not both"}}
            elif app == "failing":
                out = connected(False, "door_refused")
            else:
                out = connected()
        elif u.path == "/cli/app/db/check":
            out = connected(b.get("app") != "failing", "door_refused")
        elif u.path == "/cli/app/db/status":
            if q.get("request"):
                out = {"ok": True, "app": q["app"], "request": {"id": q["request"], "status": "approved", "result": connected()}}
            else:
                out = {"ok": True, "app": q.get("app"), "kind": "container", "running": True, "postgres": True, "majors": [16, 17],
                       "connections": [{"as": "DATABASE_URL", "db": "ws_a_app_feedback", "major": 17, "cluster": "17/main",
                                        "roles": ["ws_a_app_feedback_app_feedback__g1"], "doorOpen": True, "check": {"ok": True, "latencyMs": 3}}],
                       "pending": [{"id": "w_abc123", "summary": "k2 app db connect held → x"}]}
        elif u.path == "/cli/app/db/disconnect":
            out = {"ok": True, "app": b.get("app"), "as": "DATABASE_URL", "db": "ws_a_app_feedback", "roles": ["ws_a_app_feedback_app_feedback__g1"],
                   "rolesDropped": False, "secrets": ["app-db.feedback.DATABASE_URL"], "doorOpen": False, "sessionsCut": 1,
                   "rehost": {"result": "done"}, "note": "ws_a_app_feedback kept"}
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
WS="$FAKE_DIR/ws"
mkdir -p "$WS"

run() {
    set +e
    OUT="$("$CLI" "$@" 2>"$FAKE_DIR/stderr")"
    RC=$?
    ERR="$(cat "$FAKE_DIR/stderr")"
    set -e
}
last_req() { tail -n 1 "$FAKE_DIR/requests.log"; }
no_password() { case "$1" in *S3CRETpw*) fail "$2 printed the password: $1" ;; esac; }

# ── k2 app db ────────────────────────────────────────────────────────
help="$("$CLI" app db --help)"
for want in "k2 app db connect <app-id> --new" "--db <name|id>" "--major N" "k2 app db status" "k2 app db disconnect" "Exit codes: 0 done (PASS)" "k2 study app-databases"; do
    echo "$help" | grep -qF -- "$want" || fail "k2 app db --help lists $want"
done
# Here-strings, never `cmd | grep -q` (pipefail + SIGPIPE on a long page).
app_help="$("$CLI" app --help 2>&1)"
grep -qF 'k2 app db connect <app-id>' <<<"$app_help" || fail "k2 app --help names k2 app db"
study="$("$CLI" study app-databases)"
grep -qF 'WHAT REACHES WHAT' <<<"$study" || fail "k2 study app-databases"

: >"$FAKE_DIR/requests.log"
for args in "app db" "app db connect" "app db connect feedback" "app db connect feedback --major x --new" "app db connect a b --new" "app db frob x"; do
    # shellcheck disable=SC2086
    run $args
    if [ "$args" = "app db" ]; then
        [ "$RC" -eq 0 ] || fail "k2 app db prints its help"
        continue
    fi
    [ "$RC" -eq 1 ] || fail "k2 $args is a usage error, exit 1 (R7), got $RC: $ERR"
    echo "$ERR" | grep -q '"error":"usage"' || fail "k2 $args says usage: $ERR"
done
[ ! -s "$FAKE_DIR/requests.log" ] || fail "usage errors sent requests: $(cat "$FAKE_DIR/requests.log")"

run app db connect feedback --new --major 17 --as DATABASE_URL --reason "the feedback store"
[ "$RC" -eq 0 ] || fail "PASS exits 0, got $RC: $OUT $ERR"
grep -q '"new": true' "$FAKE_DIR/requests.log" && grep -q '"major": 17' "$FAKE_DIR/requests.log" || fail "connect sends new and major: $(cat "$FAKE_DIR/requests.log")"
echo "$OUT" | grep -q '^connected feedback → ws_a_app_feedback (Postgres 17, cluster 17/main)' || fail "connected line: $OUT"
echo "$OUT" | grep -q '^secrets  DATABASE_URL, DATABASE_MIGRATE_URL (granted to the App; not shown)' || fail "secrets by env name: $OUT"
echo "$OUT" | grep -q '^check    PASS · PostgreSQL 17.6 · 3 ms · from inside the App' || fail "PASS line: $OUT"
case "$OUT$ERR" in *postgres://*|*postgresql://*) fail "connect printed a URL: $OUT" ;; esac

run app db connect failing --db ws_a
[ "$RC" -eq 1 ] || fail "FAIL exits 1, got $RC"
echo "$OUT" | grep -q '^check    FAIL · door_refused' || fail "FAIL names the step: $OUT"
echo "$OUT" | grep -q '  | Error: connect ECONNREFUSED' || fail "FAIL shows the App's output: $OUT"

run app db connect held --new
[ "$RC" -eq 2 ] || fail "held exits 2, got $RC: $OUT $ERR"
echo "$OUT" | grep -q '^held for the owner: w_abc123 (one card)' || fail "held line: $OUT"
echo "$OUT" | grep -q 'switch off: "Build and run Apps" (Settings → Workspaces → a → Agent → Apps)' || fail "each off switch by its Settings name: $OUT"
echo "$OUT" | grep -q 'Database (what its Apps may use)' && fail "a switch that is on isn't listed as off: $OUT"
echo "$ERR" | tail -n 1 | grep -q '^Ask with: k2 permission request apps.approve request=w_abc123' || fail "held ends with its Ask with: line: $ERR"

run app db connect held --new --wait --timeout 30s
[ "$RC" -eq 0 ] || fail "--wait returns the approved connect's result, got $RC: $OUT $ERR"
echo "$OUT" | grep -q '^check    PASS' || fail "--wait prints the result: $OUT"
last_req | grep -q '"request": "w_abc123"' || fail "--wait polls that card: $(last_req)"

run app db connect nopg --new --major 16
[ "$RC" -eq 3 ] || fail "refused exits 3, got $RC"
echo "$ERR" | grep -q 'app_db_cluster_missing' || fail "names the refusal: $ERR"
echo "$ERR" | grep -q '^fix: ' || fail "an App refusal carries its fix from the registry: $ERR"

run app db connect bad --new
[ "$RC" -eq 1 ] || fail "a daemon usage refusal exits 1 (R7), got $RC: $ERR"

run app db connect feedback --new --json
[ "$RC" -eq 0 ] || fail "--json PASS exits 0"
echo "$OUT" | python3 -c '
import json, sys
v = json.load(sys.stdin)
for k in ("app", "as", "kind", "db", "cluster", "major", "roles", "secrets", "doorOpen", "check"):
    assert k in v, k
assert v["check"]["ok"] is True
' || fail "--json has the PRD shape: $OUT"

run app db status feedback
[ "$RC" -eq 0 ] || fail "status exits 0"
echo "$OUT" | grep -q 'waiting for the owner: card w_abc123' || fail "status shows the waiting card: $OUT"
echo "$OUT" | grep -q 'majors K2 runs here: 16, 17' || fail "status shows the majors from the daemon: $OUT"

run app db check failing
[ "$RC" -eq 1 ] || fail "check FAIL exits 1, got $RC"
run app db disconnect feedback
[ "$RC" -eq 0 ] || fail "disconnect exits 0, got $RC: $ERR"
echo "$OUT" | grep -q '^door     SHUT' || fail "disconnect says the door shut: $OUT"
echo "$OUT" | grep -q '1 open session(s) cut' || fail "disconnect says sessions were cut: $OUT"

echo "PASS: k2 app db connect|status|check|disconnect"
