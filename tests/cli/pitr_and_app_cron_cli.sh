#!/usr/bin/env bash
# prd-app-supabase-compat-v1 §11 (lane E): `k2 db cluster basebackup|backups|
# restore|restore-drill|drop-drill` (POST /cli/db/clusters {op}) and `k2 app
# cron add|list|run-now|logs|rm` (/cli/app/cron*). A local fake daemon logs
# each request and answers like the real routes. No real daemon.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/tests/cli/_cli_under_test.sh"; CLI="$CLI_UNDER_TEST"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-lane-e-XXXXXX)"
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
            shown = {k: v for k, v in q.items() if k != "token"}
            f.write("%s %s %s %s\n" % (self.command, u.path, json.dumps(shown, sort_keys=True), body))
        status, out = 200, {"ok": True}
        b = json.loads(body or "{}") if body else {}
        if u.path == "/cli/db/clusters" and self.command == "POST":
            op = b.get("op")
            if op == "basebackup":
                out = {"ok": True, "cluster": "17/feedback", "backup": {"stamp": "20261122T040000Z", "dir": "/var/lib/postgresql/k2-wal/17-feedback.base/20261122T040000Z",
                       "startWal": "000000010000000000000003", "endWal": "000000010000000000000003", "pruned": ["20261115T040000Z"], "kept": 7}}
            elif op == "backups":
                out = {"ok": True, "cluster": "17/feedback", "onBox": {"walArchive": True, "baseBackups": [{"stamp": "20261122T040000Z", "complete": True, "endedAt": "2026-11-22T04:00:09Z", "bytes": 1024}],
                       "wal": {"count": 9, "first": "000000010000000000000003", "last": "00000001000000000000000B", "newestAt": "2026-11-22T10:14:00Z", "gap": "000000010000000000000007"},
                       "restorableFrom": None, "restorableTo": None},
                       "recorded": {"lastBasebackupAt": "2026-11-22T04:00:09Z", "lastBasebackupOk": True, "lastDrillAt": None, "lastDrillOk": None,
                                    "drillWarning": "no good restore drill yet: run `k2 db cluster restore-drill feedback`"}}
            elif op == "restore":
                if b.get("to") == "2099-01-01T00:00:00Z":
                    status, out = 409, {"ok": False, "error": {"code": "cluster_refused", "hint": "target_after_wal: the newest WAL in the spool was archived at 2026-11-22T10:14:00Z"}}
                else:
                    out = {"ok": True, "cluster": "17/fb_old", "port": 5435, "restoredFrom": "17/feedback", "target": "2026-11-22T10:00:00Z",
                           "baseBackup": "20261122T040000Z", "next": "check it, then point the App at it; the old cluster 17/feedback is unchanged"}
            elif op == "restore-drill":
                ok = b.get("name") != "bad"
                out = {"ok": ok, "cluster": "17/%s" % b.get("name"), "target": "2026-11-22T10:15:01Z", "markers": {"before": True, "after": not ok},
                       "databases": [{"database": "app_db", "tables": 2, "rows": 5, "rowsMatch": ok, "diffs": [] if ok else [{"table": "public.orders", "atTheMoment": 3, "restored": 4}]}],
                       "missingDatabases": [], "storageCheck": "not checked", "drillCluster": "17/feedback_drill", "dropped": True}
            elif op == "drop-drill":
                out = {"ok": True, "dropped": {"major": 17, "name": b.get("name"), "dropped": True, "from": "17/feedback"}}
        elif u.path == "/cli/app/cron/add":
            out = {"ok": True, "id": "c0123456789", "app": b.get("app"), "schedule": b.get("schedule"), "path": b.get("path"), "method": (b.get("method") or "GET").upper(),
                   "next": "2026-11-22T10:15:00+00:00", "secret": {"env": "CRON_SECRET", "made": True, "rehost": {"result": "done"}}}
        elif u.path == "/cli/app/cron":
            out = {"ok": True, "app": q.get("app"), "crons": [{"id": "c0123456789", "schedule": "*/15 * * * *", "method": "GET", "path": "/api/cron/sync",
                   "next": "2026-11-22T10:15:00+00:00", "failuresInARow": 3, "last": {"at": "2026-11-22T10:00:00+00:00", "status": "failed", "http": 500}}]}
        elif u.path == "/cli/app/cron/run-now":
            out = {"ok": False, "app": b.get("app"), "id": b.get("id"), "status": "failed", "http": 500, "detail": "boom"}
        elif u.path == "/cli/app/cron/logs":
            out = {"ok": True, "app": q.get("app"), "logs": [{"id": "c0123456789", "path": "/api/cron/sync", "runs": [{"at": "2026-11-22T10:00:00+00:00", "status": "skipped_overlap", "trigger": "schedule"}]}]}
        elif u.path == "/cli/app/cron/rm":
            out = {"ok": True, "app": b.get("app"), "removed": b.get("id"), "note": "CRON_SECRET stays granted to the App"}
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
    OUT="$("$CLI" "$@" 2>&1)"
    RC=$?
    set -e
}
last_body() { tail -1 "$FAKE_DIR/requests.log" | cut -d' ' -f4-; }

# Help and study name the verbs.
help="$("$CLI" db --help)"
for v in 'cluster basebackup <name>' 'cluster backups <name>' 'cluster restore <name> --to <time> --as <new>' 'cluster restore-drill <name>' 'cluster drop-drill <name>'; do
    echo "$help" | grep -qF "$v" || fail "k2 db --help must list $v"
done
"$CLI" app cron --help | grep -q 'Authorization: Bearer \$CRON_SECRET' || fail "k2 app cron --help names the header"
"$CLI" study app-cron | grep -q 'skipped_overlap' || fail "k2 study app-cron explains overlap"
"$CLI" study | grep -q 'app-cron' || fail "k2 study lists app-cron"

# Usage errors before any request.
: >"$FAKE_DIR/requests.log"
for args in "db cluster restore feedback --to -15m" "db cluster restore feedback --as fb_old" "db cluster restore-drill feedback --as x" \
    "db cluster backups" "db cluster drop-drill feedback --major seventeen" "app cron add" "app cron add shop" "app cron add shop */5" \
    "app cron run-now shop" "app cron rm shop" "app cron frobnicate shop"; do
    # shellcheck disable=SC2086
    run $args
    [ "$RC" != 0 ] || fail "$args must be refused: $OUT"
done
[ ! -s "$FAKE_DIR/requests.log" ] || fail "usage errors send nothing: $(cat "$FAKE_DIR/requests.log")"

# PITR verbs.
run db cluster basebackup feedback
[ "$RC" = 0 ] || fail "basebackup: $OUT"
echo "$OUT" | grep -q 'base backup 20261122T040000Z of 17/feedback' || fail "basebackup output: $OUT"
last_body | grep -q '"op": *"basebackup"' || fail "basebackup body: $(last_body)"
run db cluster backups feedback
[ "$RC" = 0 ] || fail "backups: $OUT"
echo "$OUT" | grep -q 'GAP      segment 000000010000000000000007' || fail "backups shows a gap: $OUT"
echo "$OUT" | grep -q 'warning: no good restore drill yet' || fail "backups warns about drills: $OUT"
run db cluster restore feedback --to "2026-11-22 10:00" --as fb_old
[ "$RC" = 0 ] || fail "restore: $OUT"
echo "$OUT" | grep -q 'restored 17/feedback to 2026-11-22T10:00:00Z UTC as 17/fb_old (port 5435)' || fail "restore output: $OUT"
python3 -c 'import json,sys; b=json.loads(sys.argv[1]); assert b["op"]=="restore" and b["to"]=="2026-11-22 10:00" and b["as"]=="fb_old" and b["name"]=="feedback", b' "$(last_body)" || fail "restore body"
run db cluster restore feedback --to 2099-01-01T00:00:00Z --as fb_old
[ "$RC" = 3 ] || fail "a refused restore exits 3 (got $RC): $OUT"
echo "$OUT" | grep -q 'target_after_wal' || fail "refusal names the reason: $OUT"
run db cluster restore-drill feedback
[ "$RC" = 0 ] || fail "a passing drill exits 0: $OUT"
echo "$OUT" | grep -q 'PASS' || fail "drill PASS: $OUT"
run db cluster restore-drill bad --to -1h
[ "$RC" = 1 ] || fail "a failing drill exits 1 (got $RC): $OUT"
echo "$OUT" | grep -q 'public.orders: 3 at the moment, 4 restored' || fail "drill diffs: $OUT"
last_body | grep -q '"to": *"-1h"' || fail "drill --to: $(last_body)"
run db cluster drop-drill feedback_drill --major 17
[ "$RC" = 0 ] || fail "drop-drill: $OUT"
echo "$OUT" | grep -q 'removed 17/feedback_drill' || fail "drop-drill output: $OUT"

# App cron.
run app cron add shop "*/15 * * * *" --path /api/cron/sync --method post --timeout 2m
[ "$RC" = 0 ] || fail "cron add: $OUT"
echo "$OUT" | grep -q 'CRON_SECRET made and granted to the App (not shown); the App was re-hosted' || fail "cron add output: $OUT"
python3 -c 'import json,sys; b=json.loads(sys.argv[1]); assert b=={"app":"shop","schedule":"*/15 * * * *","path":"/api/cron/sync","method":"post","timeout":"2m"}, b' "$(last_body)" || fail "cron add body"
run app cron list shop
[ "$RC" = 0 ] || fail "cron list: $OUT"
echo "$OUT" | grep -q '3 failures in a row' || fail "cron list shows the streak: $OUT"
tail -1 "$FAKE_DIR/requests.log" | grep -q '^GET /cli/app/cron {"app": "shop"}' || fail "list is a GET: $(tail -1 "$FAKE_DIR/requests.log")"
run app cron run-now shop c0123456789
[ "$RC" = 1 ] || fail "a failed run-now exits 1 (got $RC): $OUT"
run app cron logs shop c0123456789 --limit 5
[ "$RC" = 0 ] || fail "cron logs: $OUT"
echo "$OUT" | grep -q 'skipped_overlap' || fail "cron logs output: $OUT"
tail -1 "$FAKE_DIR/requests.log" | grep -q '"id": "c0123456789", "limit": "5"' || fail "logs query: $(tail -1 "$FAKE_DIR/requests.log")"
run app cron rm shop c0123456789
[ "$RC" = 0 ] || fail "cron rm: $OUT"
echo "$OUT" | grep -q 'removed c0123456789 from shop' || fail "cron rm output: $OUT"
grep -q 'CRON_SECRET=' "$FAKE_DIR/requests.log" && fail "no secret value ever travels"
echo "PASS: pitr and app cron CLI"
