#!/usr/bin/env bash
# 0.46.11 (prd-app-database-integrations-v1 S0, field reports F1, F3, F4):
# `k2 db dsn` prints no password unless --reveal; `k2 db dsn --ref --db`
# works; `k2 db run` hands a command DATABASE_URL without printing it; a
# `k2 db` refusal ends with its Ask with: line. A local fake daemon stands
# in and logs each request (method, path, query without the token, body).
# No real daemon, no ~/.k2.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CLI="$ROOT/cli/k2"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-db-dsn-XXXXXX)"
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

# ── k2 db dsn: no password unless --reveal ──────────────────────────
run db dsn --workspace "$WS"
[ "$RC" -eq 0 ] || fail "k2 db dsn must exit 0, got $RC: $OUT $ERR"
no_password "$OUT$ERR" "k2 db dsn"
[ "$OUT" = "postgres://ws_a_agent@127.0.0.1:5432/ws_a" ] || fail "k2 db dsn prints the DSN without its password: $OUT"
echo "$ERR" | grep -q 'k2 db run' || fail "k2 db dsn says how to use it without the password: $ERR"
last_req | grep -q '"reveal"' && fail "plain dsn must not ask for the password: $(last_req)"

run db dsn --reveal --workspace "$WS"
[ "$RC" -eq 0 ] || fail "dsn --reveal must exit 0: $OUT $ERR"
[ "$OUT" = "postgres://ws_a_agent:S3CRETpw0123456789@127.0.0.1:5432/ws_a" ] || fail "--reveal prints the full DSN: $OUT"
echo "$ERR" | grep -q 'printed a password' || fail "--reveal warns: $ERR"
last_req | grep -q '"reveal": "1"' || fail "--reveal sends reveal=1: $(last_req)"

run db dsn --json --workspace "$WS"
no_password "$OUT" "k2 db dsn --json"
echo "$OUT" | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v["dsnHasPassword"] is False, v' || fail "--json says dsnHasPassword false: $OUT"

run db dsn --ref --reveal --workspace "$WS"
[ "$RC" -eq 2 ] || fail "--ref with --reveal is a usage error (exit 2), got $RC"

# F3: --ref --db works (K2's own hint said to run it).
run db dsn --ref --db ws_a_two --workspace "$WS"
[ "$RC" -eq 0 ] || fail "dsn --ref --db must work, got $RC: $OUT $ERR"
last_req | grep -q '"db": "ws_a_two"' && last_req | grep -q '"ref": "1"' || fail "--ref --db sends both: $(last_req)"
[ "$OUT" = "k2v://ws/a/DATABASE_URL_WS_A_TWO" ] || fail "prints the reference only: $OUT"
run db dsn --ref --db ws_a_two --as REPORTS_URL --workspace "$WS"
last_req | grep -q '"as": "REPORTS_URL"' || fail "--as is sent: $(last_req)"

# ── k2 db run: the command gets DATABASE_URL, nothing is printed ─────
run db run --workspace "$WS" -- sh -c 'test -n "$DATABASE_URL" && case "$DATABASE_URL" in *S3CRETpw*) echo got-it ;; esac'
[ "$RC" -eq 0 ] || fail "k2 db run must run the command, got $RC: $OUT $ERR"
[ "$OUT" = "got-it" ] || fail "the command got the full DSN in DATABASE_URL: $OUT"
no_password "$ERR" "k2 db run"
last_req | grep -q '"reveal": "1"' || fail "k2 db run fetches it with reveal: $(last_req)"
run db run --as PG_URL --workspace "$WS" -- sh -c 'test -n "$PG_URL" && echo named'
[ "$OUT" = "named" ] || fail "--as names the env var: $OUT $ERR"
run db run --workspace "$WS"
[ "$RC" -eq 2 ] || fail "k2 db run with no command is usage (exit 2), got $RC"

# ── F4: a k2 db refusal ends with its Ask with: line ─────────────────
run db create --workspace "$WS"
[ "$RC" -eq 3 ] || fail "refused create exits 3, got $RC: $ERR"
echo "$ERR" | grep -q '"code":"forbidden"' || fail "the error line: $ERR"
echo "$ERR" | tail -n 1 | grep -q '^Ask with: k2 permission request db.agent-access level=write' || fail "the refusal ends with its Ask with: line: $ERR"
run db grant other --db ws_a --level read --workspace "$WS"
echo "$ERR" | tail -n 1 | grep -q "^It can't be requested\.$" || fail "an unrequestable refusal says so: $ERR"


echo "PASS: k2 db dsn --reveal / --ref --db / run / Ask lines"
