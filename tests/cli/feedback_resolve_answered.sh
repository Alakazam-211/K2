#!/usr/bin/env bash
# Ticket status model (0.43.2): a person's option pick answers a ticket; a
# free-text message puts it in needs_discussion, and the AGENT settles it.
#
# Exercises the real `cli/k2` against a stub daemon that records POST bodies:
#   1. `k2 tickets resolve <id> --answered "<outcome>"` posts
#      {id, status: answered, answer, author} and prints "Marked … answered."
#   2. plain `k2 tickets resolve <id>` still posts just {id} ("Resolved …").
#   3. `--answered ""` / `--answered` with no value → usage error, exit 2,
#      nothing posted.
#   4. `k2 tickets ask … --wait` on a ticket that went to needs_discussion
#      exits 5, prints the person's latest message on stdout and
#      {"status":"needs_discussion",…} on stderr.
#   5. help teaches the free-text → needs_discussion → settle loop.
#
# No daemon build needed. Never touches ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

WORK="$(mktemp -d -t k2-fb-settle-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

python3 - "$WORK" <<'PYEOF' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

work = sys.argv[1]
counter = {"n": 0}
TICKET = "fb-stub-0000000000000001"

class H(BaseHTTPRequestHandler):
    def _send(self, resp):
        data = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        path = self.path.split("?")[0]
        counter["n"] += 1
        with open(os.path.join(work, "post-%d.json" % counter["n"]), "wb") as f:
            f.write(json.dumps({"path": path, "body": json.loads(body or b"{}")}).encode())
        if path == "/cli/feedback/resolve":
            b = json.loads(body)
            status = b.get("status", "resolved")
            resp = {"ok": True, "id": TICKET, "status": status}
            if status == "answered":
                resp["answer"] = b.get("answer")
        elif path == "/cli/feedback/create":
            resp = {"ok": True, "id": TICKET, "title": "t", "kind": "question",
                    "priority": 3, "status": "waiting"}
        else:
            resp = {"error": {"code": "not_found", "hint": "stub: unknown route"}}
        self._send(resp)

    def do_GET(self):
        path = self.path.split("?")[0]
        if path == "/cli/feedback/show":
            self._send({
                "ok": True, "id": TICKET, "status": "needs_discussion",
                "answer": None, "agentName": "scout",
                "comments": [
                    {"author": "scout", "body": "Which DB?", "at": 1},
                    {"author": "owner", "body": "what about SQLite?", "at": 2},
                    {"author": "scout", "body": "thinking", "at": 3},
                ],
            })
        else:
            self._send({"error": {"code": "not_found", "hint": "stub: unknown route"}})

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port"), "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PYEOF
STUB_PID=$!
disown "$STUB_PID"

for _ in $(seq 1 50); do
    [ -f "$WORK/stub.port" ] && break
    sleep 0.1
done
[ -f "$WORK/stub.port" ] || { echo "FAIL: stub daemon never wrote its port" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"

PASS=0
FAIL=0
pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

k2() {
    (cd "$WORK" && env -u K2_HOOK_SOCK -u K2SO_HOOK_SOCK \
        K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="stub-token" \
        K2_PROJECT_PATH="$WORK" K2_AGENT_NAME="scout" \
        "$K2_CLI" "$@")
}

posts_count() { find "$WORK" -name 'post-*.json' | wc -l | tr -d ' '; }
last_post() { python3 -c 'import json,sys,glob
files = sorted(glob.glob(sys.argv[1] + "/post-*.json"), key=lambda p: int(p.rsplit("-",1)[1].split(".")[0]))
print(open(files[-1]).read())' "$WORK"; }

# 1. resolve --answered posts status answered + answer + agent author.
out="$(k2 tickets resolve fb-stub --answered "SQLite now, Postgres later" 2>&1)" || { fail "case 1 exited non-zero: $out"; out=""; }
got="$(last_post)"
if python3 -c 'import json,sys
d = json.loads(sys.argv[1])
assert d["path"] == "/cli/feedback/resolve", d
b = d["body"]
assert b == {"id": "fb-stub", "status": "answered", "answer": "SQLite now, Postgres later", "author": "scout"}, b' "$got"; then
    pass "case 1 — --answered posts {id,status:answered,answer,author}"
else
    fail "case 1 — wrong payload: $got"
fi
case "$out" in
    *"Marked fb-stub- answered."*) pass "case 1 — prints Marked … answered." ;;
    *) fail "case 1 — output: $out" ;;
esac

# 2. plain resolve still posts only the id.
out="$(k2 tickets resolve fb-stub 2>&1)" || { fail "case 2 exited non-zero: $out"; out=""; }
got="$(last_post)"
if python3 -c 'import json,sys
d = json.loads(sys.argv[1]); assert d["body"] == {"id": "fb-stub"}, d' "$got"; then
    pass "case 2 — plain resolve posts {id}"
else
    fail "case 2 — wrong payload: $got"
fi
case "$out" in
    *"Resolved fb-stub-."*) pass "case 2 — prints Resolved …" ;;
    *) fail "case 2 — output: $out" ;;
esac

# 3. empty / missing outcome → exit 2, nothing posted.
before="$(posts_count)"
set +e
k2 tickets resolve fb-stub --answered "   " >/dev/null 2>"$WORK/err3"; rc_blank=$?
k2 tickets resolve fb-stub --answered >/dev/null 2>"$WORK/err3b"; rc_missing=$?
set -e
after="$(posts_count)"
if [ "$rc_blank" -eq 2 ] && [ "$rc_missing" -eq 2 ] && [ "$before" = "$after" ] \
        && grep -q '"code": "usage"\|"code":"usage"' "$WORK/err3"; then
    pass "case 3 — blank/missing --answered is a usage error (exit 2), nothing posted"
else
    fail "case 3 — rc_blank=$rc_blank rc_missing=$rc_missing posts $before→$after err=$(cat "$WORK/err3")"
fi

# 4. ask --wait on a needs_discussion ticket → exit 5 + the person's message.
set +e
k2 tickets ask "Which DB?" --assign owner --wait --timeout 10 >"$WORK/out4" 2>"$WORK/err4"; rc=$?
set -e
if [ "$rc" -eq 5 ] && [ "$(cat "$WORK/out4")" = "what about SQLite?" ] \
        && grep -q '"status": "needs_discussion"' "$WORK/err4"; then
    pass "case 4 — --wait exits 5 with the free-text message"
else
    fail "case 4 — rc=$rc stdout=$(cat "$WORK/out4") stderr=$(cat "$WORK/err4")"
fi

# 5. help teaches the loop.
help="$(k2 tickets --help 2>&1)"
resolve_help="$(k2 tickets resolve --help 2>&1)"
if printf '%s' "$help" | grep -q 'needs_discussion' \
        && printf '%s' "$resolve_help" | grep -q -- '--answered <outcome>'; then
    pass "case 5 — help teaches needs_discussion + --answered"
else
    fail "case 5 — help text missing the settle loop"
fi

echo
echo "feedback_resolve_answered: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
