#!/usr/bin/env bash
# `k2 tickets settings` (Settings → Tickets, 0.45.4).
#
# Runs the REAL cli/k2 against a stub python daemon that records every
# /cli/tickets/settings request (method, token, body):
#   1. no flags → GET, prints the three rules, exit 0.
#   2. --brief/--assignee-required/--default-assignee → POST with exactly
#      those keys, prints "Saved.", exit 0.
#   3. inside a K2 terminal (K2_HOOK_SOCK set) a read uses the owner token
#      (tickets is an Open tool) but a WRITE sends the session passport;
#      the daemon's owner_only refusal exits 3 with its hint on stderr.
#   4. a bad rule value exits 2 and sends nothing.
#   5. --json prints the daemon's object.
#   6. a bad_value / unknown_user refusal exits 2.
#   7. help, the ticket-brief study page and --schema teach the verb.
#
# No daemon build. Never touches ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

WORK="$(mktemp -d -t k2-tk-settings-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

python3 - "$WORK" <<'PYEOF' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

work = sys.argv[1]
state = {"briefPolicy": "warn", "assigneePolicy": "warn", "defaultAssignee": None}
log = os.path.join(work, "requests.jsonl")

def view():
    d = dict(state)
    d.update({"ok": True, "defaultAssigneeKnown": True, "canEdit": True,
              "values": ["off", "warn", "require"],
              "people": [{"value": "owner", "label": "Owner", "role": "owner", "disabled": False}]})
    return d

class H(BaseHTTPRequestHandler):
    def reply(self, status, obj):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def record(self, method, body):
        u = urlsplit(self.path)
        token = (parse_qs(u.query).get("token") or [""])[0]
        with open(log, "a") as f:
            f.write(json.dumps({"method": method, "path": u.path, "token": token,
                                "body": body.decode() if body else ""}) + "\n")
        return u.path, token

    def do_GET(self):
        path, _ = self.record("GET", b"")
        if path != "/cli/tickets/settings":
            return self.reply(404, {"error": {"code": "not_found", "hint": "stub"}})
        self.reply(200, view())

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        path, token = self.record("POST", body)
        if path != "/cli/tickets/settings":
            return self.reply(404, {"error": {"code": "not_found", "hint": "stub"}})
        if token == "passport-xyz":
            return self.reply(403, {"ok": False, "error": {"code": "owner_only",
                "hint": "Ticket rules are owner settings: an agent can't change them."}})
        req = json.loads(body)
        if req.get("defaultAssignee") == "ghost":
            return self.reply(400, {"ok": False, "error": {"code": "unknown_user",
                "hint": "`ghost` is not a user on this server."}})
        for k in ("briefPolicy", "assigneePolicy"):
            if k in req:
                state[k] = req[k]
        if "defaultAssignee" in req:
            state["defaultAssignee"] = req["defaultAssignee"] or None
        self.reply(200, view())

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
ok()  { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

# An empty HOME with only an owner token on disk (what an in-cell Open
# tool re-sources).
FAKE_HOME="$WORK/home"
mkdir -p "$FAKE_HOME/.k2"
printf 'owner-disk-token' > "$FAKE_HOME/.k2/heartbeat.token"

RC=0
# Outside K2: the owner token in the environment.
k2_outside() {
    set +e
    (cd "$WORK" && env -i PATH="$PATH" HOME="$FAKE_HOME" \
        K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="owner-env-token" K2_PROJECT_PATH="$WORK" \
        "$K2_CLI" "$@") >"$WORK/out" 2>"$WORK/err"
    RC=$?
    set -e
}
# Inside a K2 terminal: a session passport + hook socket.
k2_inside() {
    set +e
    (cd "$WORK" && env -i PATH="$PATH" HOME="$FAKE_HOME" \
        K2_PORT="$STUB_PORT" K2_HOOK_TOKEN="passport-xyz" K2_HOOK_SOCK="$WORK/no.sock" \
        K2_PROJECT_PATH="$WORK" \
        "$K2_CLI" "$@") >"$WORK/out" 2>"$WORK/err"
    RC=$?
    set -e
}
last_req() { tail -n 1 "$WORK/requests.jsonl"; }
req_field() { last_req | python3 -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"; }
req_count() { [ -f "$WORK/requests.jsonl" ] && wc -l < "$WORK/requests.jsonl" | tr -d ' ' || echo 0; }

# 1. Show.
k2_outside tickets settings
if [ "$RC" -eq 0 ] && grep -q "Require an HTML brief:  warn" "$WORK/out" \
    && grep -q "Require an assignee:    warn" "$WORK/out" \
    && grep -q "Default assignee:       none" "$WORK/out" \
    && [ "$(req_field method)" = "GET" ] && [ "$(req_field token)" = "owner-env-token" ]; then
    ok "show: GET with the owner token, three rules printed"
else
    bad "show: rc=$RC out=$(cat "$WORK/out") err=$(cat "$WORK/err")"
fi

# 2. Change.
k2_outside tickets settings --brief require --assignee-required off --default-assignee none
body="$(req_field body)"
if [ "$RC" -eq 0 ] && grep -q "^Saved." "$WORK/out" && grep -q "Require an HTML brief:  require" "$WORK/out" \
    && [ "$(req_field method)" = "POST" ] \
    && python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d=={"briefPolicy":"require","assigneePolicy":"off","defaultAssignee":"none"}, d' "$body"; then
    ok "change: POST with exactly the given keys (none passes through; the daemon clears)"
else
    bad "change: rc=$RC body=$body out=$(cat "$WORK/out") err=$(cat "$WORK/err")"
fi
k2_outside tickets settings --default-assignee owner
if [ "$RC" -eq 0 ] && grep -q "Default assignee:       owner" "$WORK/out" \
    && [ "$(req_field body)" = '{"defaultAssignee": "owner"}' ]; then
    ok "change: default assignee only"
else
    bad "default assignee: rc=$RC body=$(req_field body) out=$(cat "$WORK/out")"
fi

# 3. Inside a K2 terminal: read with the owner token, write with the passport.
k2_inside tickets settings
if [ "$RC" -eq 0 ] && [ "$(req_field token)" = "owner-disk-token" ]; then
    ok "in-cell read: Open tool uses the owner token"
else
    bad "in-cell read: rc=$RC token=$(req_field token) err=$(cat "$WORK/err")"
fi
k2_inside tickets settings --brief off
if [ "$RC" -eq 3 ] && [ "$(req_field token)" = "passport-xyz" ] && [ "$(req_field method)" = "POST" ] \
    && grep -q '"code": "owner_only"' "$WORK/err" && [ ! -s "$WORK/out" ]; then
    ok "in-cell write: sends the passport, owner_only exits 3"
else
    bad "in-cell write: rc=$RC token=$(req_field token) err=$(cat "$WORK/err") out=$(cat "$WORK/out")"
fi

# 4. Bad value: exit 2, nothing sent.
before="$(req_count)"
k2_outside tickets settings --brief sometimes
if [ "$RC" -eq 2 ] && [ "$(req_count)" = "$before" ] && grep -q '"code": "usage"' "$WORK/err"; then
    ok "bad rule value: exit 2, no request"
else
    bad "bad value: rc=$RC err=$(cat "$WORK/err")"
fi
k2_outside tickets settings --nope
if [ "$RC" -eq 2 ] && [ "$(req_count)" = "$before" ]; then ok "unknown flag: exit 2"; else bad "unknown flag: rc=$RC"; fi

# 5. --json.
k2_outside tickets settings --json
if [ "$RC" -eq 0 ] && python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["briefPolicy"]=="require" and d["canEdit"] is True, d' "$WORK/out"; then
    ok "--json prints the daemon object"
else
    bad "--json: rc=$RC out=$(cat "$WORK/out")"
fi

# 6. Daemon refusals of a value exit 2.
k2_outside tickets settings --default-assignee ghost
if [ "$RC" -eq 2 ] && grep -q '"code": "unknown_user"' "$WORK/err"; then
    ok "unknown_user: exit 2"
else
    bad "unknown_user: rc=$RC err=$(cat "$WORK/err")"
fi

# 7. Teaching.
k2_outside tickets settings --help
if [ "$RC" -eq 0 ] && grep -q -- "--assignee-required" "$WORK/out" && grep -q "owner_only" "$WORK/out"; then
    ok "settings --help"
else
    bad "settings --help: rc=$RC"
fi
k2_outside tickets --help
if grep -q "settings" "$WORK/out"; then ok "tickets --help lists settings"; else bad "tickets --help"; fi
k2_outside study ticket-brief
if grep -q "THE RULES ARE OWNER SETTINGS (Settings → Tickets)" "$WORK/out" && grep -q "k2 tickets settings" "$WORK/out"; then
    ok "study ticket-brief: the rules are owner settings"
else
    bad "study ticket-brief: $(head -20 "$WORK/out")"
fi
k2_outside --schema
if python3 - "$WORK/out" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
cmds = {c["name"]: c for c in d["commands"]}
s = cmds["feedback settings"]
names = {f["name"] for f in s["flags"]}
assert {"--brief", "--assignee-required", "--default-assignee", "--json"} <= names, names
assert "owner_only" in s["description"], s["description"]
PY
then ok "--schema lists feedback settings"; else bad "--schema"; fi

echo ""
echo "tickets_settings: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
