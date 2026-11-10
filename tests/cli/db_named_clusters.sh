#!/usr/bin/env bash
# 0.46.11 (prd-named-pg-clusters-v1): `k2 db cluster create|restart`,
# `k2 db create --cluster` and `k2 db clusters --health`. A local fake
# daemon stands in: it logs each request (method, path, body) and answers
# POST /cli/db/clusters like the real route (create, restart, create-db;
# a refusal for 17/k2db, a cluster made outside K2). No real daemon.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CLI="$ROOT/cli/k2"
source "$ROOT/tests/cli/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

FAKE_DIR="$(mktemp -d -t k2-db-named-XXXXXX)"
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
FB = {"major": 17, "name": "feedback", "label": "17/feedback", "port": 5434, "unit": "postgresql@17-feedback.service",
      "role": "k2", "origin": "created", "state": "online", "named": True, "walArchive": True,
      "walSpool": "/var/lib/postgresql/k2-wal/17-feedback"}
K2DB = {"major": 17, "name": "k2db", "label": "17/k2db", "port": 5433, "unit": "postgresql@17-k2db.service",
        "role": "foreign", "origin": "adopted", "state": "online", "named": True}
MAIN = {"major": 16, "name": "main", "label": "16/main", "port": 5432, "unit": "postgresql@16-main.service",
        "role": "k2", "origin": "baked", "state": "online", "named": False}
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
        if u.path == "/cli/db/status":
            out = {"ok": True, "clusters": [MAIN, K2DB, FB], "defaultMajor": 16,
                   "databases": [{"name": "ws_a", "major": 16}, {"name": "feedback", "major": 17, "cluster": "17/feedback"}],
                   "upgradeAvailable": []}
        elif u.path == "/cli/db/clusters" and self.command == "GET":
            rows = [dict(MAIN), dict(K2DB), dict(FB)]
            if q.get("health") == "1":
                rows[0]["health"] = {"online": True, "connections": 2, "maxConnections": 50, "databases": [{"name": "ws_a", "bytes": 8388608}], "archiveMode": "off"}
                rows[2]["health"] = {"online": True, "connections": 3, "maxConnections": 50, "databases": [{"name": "feedback", "bytes": 9437184}],
                                     "archiveMode": "on", "archiver": {"archivedCount": 7, "lastArchivedAt": "2026-11-10T01:00:00Z", "failedCount": 0, "lastFailedAt": None}}
            out = {"ok": True, "clusters": rows}
        elif u.path == "/cli/db/clusters":
            b = json.loads(body or "{}")
            if b.get("name") == "k2db" or b.get("cluster") == "k2db":
                status, out = 409, {"ok": False, "error": {"code": "cluster_refused", "hint": "17/k2db on this box is a cluster K2 didn't make"}}
            elif b.get("op") == "create":
                out = {"ok": True, "cluster": FB}
            elif b.get("op") == "restart":
                out = {"ok": True, "cluster": "17/feedback", "restart": {"unit": FB["unit"]}}
            elif b.get("op") == "create-db":
                out = {"ok": True, "name": b.get("name"), "cluster": "17/feedback", "major": 17,
                       "dsn": "postgres://feedback_agent@127.0.0.1:5434/feedback", "dsnHint": "k2 db dsn --db feedback"}
            else:
                status, out = 400, {"ok": False, "error": {"code": "usage", "hint": "unknown op"}}
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
    OUT="$("$CLI" "$@" 2>&1)"
    RC=$?
    set -e
}

# Help and study name the verbs.
help="$("$CLI" db --help)"
echo "$help" | grep -q 'cluster create <M> --name <n>' || fail "k2 db --help must list cluster create"
echo "$help" | grep -q 'cluster restart <name>' || fail "k2 db --help must list cluster restart"
echo "$help" | grep -q 'create --cluster <name>' || fail "k2 db --help must list create --cluster"
"$CLI" study db-upgrade | grep -q 'NAMED CLUSTERS' || fail "k2 study db-upgrade must explain named clusters"

# Usage errors exit 2 before any request.
: >"$FAKE_DIR/requests.log"
for args in "db cluster" "db cluster create" "db cluster create 17" "db cluster create seventeen --name feedback" \
    "db cluster create 17 --name feedback --port 80" "db cluster restart" "db cluster drop feedback" \
    "db create --cluster feedback --test" "db create --cluster feedback --db-access write"; do
    # shellcheck disable=SC2086
    run $args --workspace "$WS"
    [ "$RC" -eq 2 ] || fail "k2 $args must exit 2, got $RC: $OUT"
done
[ ! -s "$FAKE_DIR/requests.log" ] || fail "usage errors sent requests: $(cat "$FAKE_DIR/requests.log")"

# create: one POST with the op, the major, the name, the port, the archive flag.
run db cluster create 17 --name feedback --port 5434 --wal-archive --workspace "$WS"
[ "$RC" -eq 0 ] || fail "cluster create must exit 0, got $RC: $OUT"
echo "$OUT" | grep -q 'cluster 17/feedback on port 5434' || fail "create must print the cluster: $OUT"
echo "$OUT" | grep -q 'WAL archive: /var/lib/postgresql/k2-wal/17-feedback' || fail "create must print the spool: $OUT"
echo "$OUT" | grep -q 'next: k2 db create --cluster feedback' || fail "create must say what's next: $OUT"
grep -q '^POST /cli/db/clusters .*"op": *"create"' "$FAKE_DIR/requests.log" || fail "create op: $(cat "$FAKE_DIR/requests.log")"
grep -q '"major": *17' "$FAKE_DIR/requests.log" && grep -q '"port": *5434' "$FAKE_DIR/requests.log" && grep -q '"walArchive": *true' "$FAKE_DIR/requests.log" \
    || fail "create body: $(cat "$FAKE_DIR/requests.log")"

# A cluster made outside K2: exit 3 with the refusal on stderr.
run db cluster create 17 --name k2db --workspace "$WS"
[ "$RC" -eq 3 ] || fail "17/k2db must exit 3, got $RC: $OUT"
echo "$OUT" | grep -q '"code": *"cluster_refused"' || fail "17/k2db refusal: $OUT"
run db create --cluster k2db --name x --workspace "$WS"
[ "$RC" -eq 3 ] || fail "create --cluster k2db must exit 3, got $RC: $OUT"

# restart and create --cluster.
: >"$FAKE_DIR/requests.log"
run db cluster restart feedback --workspace "$WS"
[ "$RC" -eq 0 ] || fail "restart must exit 0, got $RC: $OUT"
echo "$OUT" | grep -q 'restarted 17/feedback' || fail "restart output: $OUT"
grep -q '"op": *"restart".*"name": *"feedback"\|"name": *"feedback".*"op": *"restart"' "$FAKE_DIR/requests.log" || fail "restart body: $(cat "$FAKE_DIR/requests.log")"
run db create --cluster feedback --name feedback --major 17 --workspace "$WS"
[ "$RC" -eq 0 ] || fail "create --cluster must exit 0, got $RC: $OUT"
echo "$OUT" | grep -q 'created feedback in 17/feedback' || fail "create --cluster output: $OUT"
grep -q '"op": *"create-db"' "$FAKE_DIR/requests.log" && grep -q '"cluster": *"feedback"' "$FAKE_DIR/requests.log" \
    || fail "create-db body: $(cat "$FAKE_DIR/requests.log")"
run db create --cluster feedback --name feedback --workspace "$WS" --json
echo "$OUT" | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v["cluster"]=="17/feedback", v' || fail "create --cluster --json: $OUT"

# clusters: every cluster with its label; 17/k2db says it is never touched;
# --health adds the monitor's lines.
run db clusters --workspace "$WS"
[ "$RC" -eq 0 ] || fail "clusters must exit 0, got $RC: $OUT"
echo "$OUT" | grep -q '17/feedback .*port 5434.*1 database(s)  named, WAL archive' || fail "clusters must list 17/feedback: $OUT"
echo "$OUT" | grep -q '17/k2db .*foreign.*made outside K2: never touched' || fail "clusters must flag 17/k2db: $OUT"
echo "$OUT" | grep -q '16/main .*1 database(s)$' || fail "clusters must count 16/main's own database only: $OUT"
run db clusters --health --workspace "$WS"
echo "$OUT" | grep -q '3/50 connections · feedback 9 MB' || fail "health line: $OUT"
echo "$OUT" | grep -q 'WAL archive: 7 archived' || fail "archive line: $OUT"
grep -q '^GET /cli/db/clusters {"health": "1"}' "$FAKE_DIR/requests.log" || fail "--health must ask health=1: $(cat "$FAKE_DIR/requests.log")"
run db clusters --health --json --workspace "$WS"
echo "$OUT" | python3 -c 'import json,sys; v=json.load(sys.stdin); assert any(c.get("health") for c in v["clusters"]), v' || fail "clusters --json: $OUT"

echo "PASS: k2 db named clusters"
