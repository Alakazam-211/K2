#!/usr/bin/env bash
# `k2 tickets` in an agent's session sends the session passport, never the
# owner token (D1b: the daemon takes the passport as the agent, on its own
# workspace and tickets).
#
# Runs the REAL CLI against a stub python daemon that records every request
# (method, path, the Bearer header and any token= in the query). A fake HOME
# (and K2_HOME) holds an owner token on disk, the token an Open verb used to
# re-source in a session.
#   1. In a session (K2_HOOK_SOCK set, a passport in K2_HOOK_TOKEN): ask,
#      ask --wait, list, show, comment, resolve, resolve --answered, assign,
#      settings and a settings change each reach the stub, and every request
#      carries the passport; none carries the owner token.
#   2. In a session with no passport: each verb prints one line on stderr,
#      exits 3 and sends nothing.
#   3. Help and `template` in that session make no request and exit 0.
#   4. Outside a session the owner token on disk is sent, as before (so the
#      stub really sees it when it is sent).
#   5. A refusal that isn't the agent's to get past (own_workspace_only,
#      person_only, not_this_agent) exits 3 on every verb that can meet one,
#      with the daemon's error on stderr and nothing on stdout.
#   6. In a session (with K2_AGENT_NAME and K2_SESSION_ID set) ask sends no
#      agentName and no sessionId but keeps sessionKind, and comment and
#      resolve --answered send no author: K2's server stamps the passport's
#      own and refuses another (not_this_agent). Outside a session they go
#      as before.
#
# Bash 3.2 safe. No daemon build. Never touches ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2_CLI="$CLI_UNDER_TEST"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 1; }

WORK="$(mktemp -d -t k2-tk-passport-XXXXXX)"
STUB_PID=""
cleanup() {
    [ -n "$STUB_PID" ] && kill "$STUB_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

OWNER_TOKEN="owner-disk-token-d1b"
PASSPORT="11111111-2222-3333-4444-555555555555.passportsecret"

python3 - "$WORK" <<'PYEOF' &
import json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit, parse_qs

work = sys.argv[1]
log = os.path.join(work, "requests.jsonl")
TID = "fb0d1b00-0000-4000-8000-000000000001"
ITEM = {"ok": True, "id": TID, "title": "Ship it?", "kind": "question", "priority": 3,
        "status": "answered", "answer": "yes", "agentName": "agent", "assignees": ["owner"],
        "comments": [], "warnings": [], "attachments": []}

class H(BaseHTTPRequestHandler):
    def reply(self, obj, status=200):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def record(self, method, body):
        u = urlsplit(self.path)
        auth = self.headers.get("Authorization") or ""
        with open(log, "a") as f:
            f.write(json.dumps({"method": method, "path": u.path, "auth": auth,
                                "query_token": parse_qs(u.query).get("token", []),
                                "body": body.decode("utf-8", "replace")}) + "\n")
        return u.path, parse_qs(u.query), body

    def answer(self, path, query, body):
        # Refusal rows: a marker in the request picks the daemon's refusal.
        try:
            req = json.loads(body) if body else {}
        except ValueError:
            req = {}
        marked = (query.get("id") == ["refused1"] or req.get("id") == "refused1"
                  or req.get("title") == "refused" or query.get("status") == ["planned"])
        if req.get("title") == "notme":
            return self.reply({"ok": False, "error": {"code": "not_this_agent", "hint": "stub refusal", "field": "agentName"}}, 403)
        if marked:
            code = "person_only" if path == "/cli/feedback/resolve" else "own_workspace_only"
            return self.reply({"ok": False, "error": {"code": code, "hint": "stub refusal"}}, 403)
        if path == "/cli/feedback/list":
            return self.reply({"ok": True, "items": [ITEM]})
        if path in ("/cli/feedback/show", "/cli/feedback/create", "/cli/feedback/resolve"):
            return self.reply(ITEM)
        if path == "/cli/feedback/comment":
            return self.reply({"ok": True, "id": TID, "commentId": "c1", "author": "agent", "attachments": []})
        if path == "/cli/feedback/assign":
            return self.reply({"ok": True, "id": TID, "assignees": ["owner"], "warnings": []})
        if path == "/cli/tickets/settings":
            return self.reply({"ok": True, "briefPolicy": "warn", "assigneePolicy": "warn",
                               "defaultAssignee": None, "defaultAssigneeKnown": True,
                               "canEdit": False, "values": ["off", "warn", "require"], "people": []})
        if path == "/cli/whoami":
            return self.reply({"ok": True})
        return self.reply({"error": {"code": "not_found", "hint": "stub: " + path}}, 404)

    def do_GET(self):
        self.answer(*self.record("GET", b""))

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.answer(*self.record("POST", body))

    def log_message(self, *a):
        pass

srv = HTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(work, "stub.port.tmp"), "w") as f:
    f.write(str(srv.server_address[1]))
os.rename(os.path.join(work, "stub.port.tmp"), os.path.join(work, "stub.port"))
srv.serve_forever()
PYEOF
STUB_PID=$!
# No job notice (bash 3.2 prints the stub's source when cleanup kills it).
disown "$STUB_PID" 2>/dev/null || true

for _ in $(seq 1 100); do
    [ -s "$WORK/stub.port" ] && break
    sleep 0.05
done
[ -s "$WORK/stub.port" ] || { echo "FAIL: the stub daemon never wrote its port" >&2; exit 1; }
STUB_PORT="$(cat "$WORK/stub.port")"
: >"$WORK/requests.jsonl"

FAKE_HOME="$WORK/home"
mkdir -p "$FAKE_HOME/.k2" "$WORK/proj"
printf '%s' "$OWNER_TOKEN" >"$FAKE_HOME/.k2/heartbeat.token"
printf '%s' "$OWNER_TOKEN" >"$FAKE_HOME/.k2/daemon.token"
printf '%s' "$STUB_PORT" >"$FAKE_HOME/.k2/heartbeat.port"
printf 'a brief file\n' >"$WORK/proj/note.txt"

PASS=0
FAIL=0
ok()  { echo "PASS: $1"; PASS=$((PASS + 1)); }
bad() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

RC=0
# run <mode> <args…>: mode is session (a passport), named (a passport, with
# an agent name, a session id and a cell set as a session has them), bare (a
# session with no passport), outside or outside_named (no session). Only the
# variables named here reach the CLI (env -i), so the caller's own K2_* never do.
run() {
    local mode="$1"; shift
    local -a envs=(PATH="$PATH" HOME="$FAKE_HOME" K2_HOME="$FAKE_HOME/.k2"
        K2_PORT="$STUB_PORT" K2_PROJECT_PATH="$WORK/proj")
    case "$mode" in
        session) envs+=(K2_HOOK_SOCK="$WORK/no.sock" K2_HOOK_TOKEN="$PASSPORT") ;;
        named) envs+=(K2_HOOK_SOCK="$WORK/no.sock" K2_HOOK_TOKEN="$PASSPORT"
            K2_AGENT_NAME="named-agent" K2_SESSION_ID="sess-d1b" K2_CELL="canonical") ;;
        outside_named) envs+=(K2_AGENT_NAME="named-agent" K2_SESSION_ID="sess-d1b" K2_CELL="canonical") ;;
        bare) envs+=(K2_HOOK_SOCK="$WORK/no.sock") ;;
        outside) ;;
        *) echo "FAIL: unknown mode $mode" >&2; exit 1 ;;
    esac
    set +e
    (cd "$WORK/proj" && env -i "${envs[@]}" "$K2_CLI" "$@") >"$WORK/out" 2>"$WORK/err" </dev/null
    RC=$?
    set -e
}
count() { wc -l <"$WORK/requests.jsonl" | tr -d ' '; }
# Every request from line $1 on carries exactly the credential $2 (Bearer
# header or token=) and never the other one, $3. Prints the paths seen.
check_creds() {
    python3 - "$WORK/requests.jsonl" "$1" "$2" "$3" <<'PY'
import json, sys
path, start, want, never = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
rows = [json.loads(l) for l in open(path).read().splitlines()[start:]]
if not rows:
    sys.exit("no request reached the stub")
for r in rows:
    sent = ([r["auth"][len("Bearer "):]] if r["auth"].startswith("Bearer ") else []) + r["query_token"]
    if never in sent or never in r["auth"]:
        sys.exit("%s %s carried the owner token" % (r["method"], r["path"]))
    if sent != [want]:
        sys.exit("%s %s carried %r, not the expected credential" % (r["method"], r["path"], sent))
print(" ".join("%s:%s" % (r["method"], r["path"]) for r in rows))
PY
}

# 1. In a session: every tickets verb sends the passport.
check_session() {
    local label="$1" want_path="$2"; shift 2
    local before seen
    before="$(count)"
    run session "$@"
    if [ "$RC" -ne 0 ]; then
        bad "$label: exit $RC, err=$(cat "$WORK/err")"
        return
    fi
    if ! seen="$(check_creds "$before" "$PASSPORT" "$OWNER_TOKEN" 2>&1)"; then
        bad "$label: $seen"
        return
    fi
    case " $seen " in
        *" $want_path "*) ok "$label sends the passport ($seen)" ;;
        *) bad "$label: expected $want_path, saw $seen" ;;
    esac
}
check_session "ask" "POST:/cli/feedback/create" tickets ask "Ship it?" --kind fyi
check_session "ask --wait" "GET:/cli/feedback/show" tickets ask "Ship it?" --kind fyi --wait --timeout 10
check_session "list" "GET:/cli/feedback/list" tickets list
check_session "list --all" "GET:/cli/feedback/list" feedback list --all
check_session "show" "GET:/cli/feedback/show" tickets show fb0d1b00
check_session "comment" "POST:/cli/feedback/comment" tickets comment fb0d1b00 "Looking now"
check_session "comment --file" "POST:/cli/feedback/comment" tickets comment fb0d1b00 --file note.txt
check_session "resolve" "POST:/cli/feedback/resolve" tickets resolve fb0d1b00
check_session "resolve --answered" "POST:/cli/feedback/resolve" tickets resolve fb0d1b00 --answered "Shipped"
check_session "assign" "POST:/cli/feedback/assign" tickets assign fb0d1b00 owner
check_session "settings" "GET:/cli/tickets/settings" tickets settings
check_session "settings --json" "GET:/cli/tickets/settings" tickets settings --json
check_session "settings change" "POST:/cli/tickets/settings" tickets settings --brief off

# 2. A session with no passport: one line, exit 3, nothing sent.
for args in "tickets ask Ship" "tickets list" "feedback list" "tickets show fb0d1b00" \
    "tickets comment fb0d1b00 hi" "tickets resolve fb0d1b00" "tickets assign fb0d1b00 owner" \
    "tickets settings" "tickets settings --brief off"; do
    before="$(count)"
    # shellcheck disable=SC2086 # split the case into words on purpose
    run bare $args
    lines="$(wc -l <"$WORK/err" | tr -d ' ')"
    if [ "$RC" -eq 3 ] && [ "$lines" = 1 ] && [ ! -s "$WORK/out" ] && [ "$(count)" = "$before" ] \
        && grep -q "passport" "$WORK/err"; then
        ok "no passport: '$args' exits 3, one line, nothing sent"
    else
        bad "no passport: '$args' rc=$RC lines=$lines requests=$(( $(count) - before )) err=$(cat "$WORK/err") out=$(cat "$WORK/out")"
    fi
done

# 3. Help and the template make no request, passport or not.
for args in "tickets ask --help" "tickets comment fb0d1b00 -h" "tickets settings --help" "tickets template" "tickets --help"; do
    before="$(count)"
    # shellcheck disable=SC2086
    run bare $args
    if [ "$RC" -eq 0 ] && [ -s "$WORK/out" ] && [ "$(count)" = "$before" ]; then
        ok "no passport: '$args' prints and sends nothing"
    else
        bad "no passport: '$args' rc=$RC requests=$(( $(count) - before )) err=$(cat "$WORK/err")"
    fi
done

# 5. Refused (own_workspace_only, or person_only on resolve): exit 3, the
#    daemon's error on stderr, nothing on stdout.
refused_row() {
    local label="$1" code="$2"; shift 2
    run session "$@"
    if [ "$RC" -eq 3 ] && [ ! -s "$WORK/out" ] && grep -q "\"code\": \"$code\"" "$WORK/err"; then
        ok "refused: $label exits 3 ($code)"
    else
        bad "refused: $label rc=$RC err=$(cat "$WORK/err") out=$(cat "$WORK/out")"
    fi
}
refused_row "ask" own_workspace_only tickets ask refused --kind fyi
refused_row "list" own_workspace_only tickets list --status planned
refused_row "show" own_workspace_only tickets show refused1
refused_row "comment" own_workspace_only tickets comment refused1 "hi"
refused_row "resolve" person_only tickets resolve refused1
refused_row "resolve --answered" person_only tickets resolve refused1 --answered "done"
refused_row "assign" own_workspace_only tickets assign refused1 owner
refused_row "ask (another agent)" not_this_agent tickets ask notme --kind fyi

# 6. The identity fields: none in a session, as before outside one.
# body_row <label> <mode> <path> <python check on the body `b`> <args…>
body_row() {
    local label="$1" mode="$2" want_path="$3" check="$4"; shift 4
    local before why
    before="$(count)"
    run "$mode" "$@"
    if [ "$RC" -ne 0 ]; then
        bad "$label: exit $RC, err=$(cat "$WORK/err")"
        return
    fi
    if why="$(python3 - "$WORK/requests.jsonl" "$before" "$want_path" "$check" <<'PY'
import json, sys
path, start, want, check = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
rows = [json.loads(l) for l in open(path).read().splitlines()[start:]]
hit = [r for r in rows if r["method"] == "POST" and r["path"] == want]
if len(hit) != 1:
    sys.exit("expected one POST %s, saw %s" % (want, [(r["method"], r["path"]) for r in rows]))
b = json.loads(hit[0]["body"])
if not eval(check, {"b": b}):
    sys.exit("body %s fails: %s" % (json.dumps(b, sort_keys=True), check))
PY
)"; then
        ok "$label"
    else
        bad "$label: $why"
    fi
}
body_row "in a session, ask sends no agentName or sessionId, and keeps sessionKind" named /cli/feedback/create \
    '"agentName" not in b and "sessionId" not in b and b.get("sessionKind") == "canonical"' tickets ask "Ship it?" --kind fyi
body_row "in a session, comment sends no author" named /cli/feedback/comment \
    '"author" not in b and b.get("body") == "Looking now"' tickets comment fb0d1b00 "Looking now"
body_row "in a session, resolve --answered sends no author" named /cli/feedback/resolve \
    '"author" not in b and b.get("status") == "answered" and b.get("answer") == "Shipped"' tickets resolve fb0d1b00 --answered "Shipped"
body_row "outside a session, ask sends agentName and sessionId as before" outside_named /cli/feedback/create \
    'b.get("agentName") == "named-agent" and b.get("sessionId") == "sess-d1b" and b.get("sessionKind") == "canonical"' tickets ask "Ship it?" --kind fyi
body_row "outside a session, comment sends its author as before" outside_named /cli/feedback/comment \
    'b.get("author") == "named-agent"' tickets comment fb0d1b00 "Looking now"

# 4. Outside a session the owner token on disk is what goes, as before.
before="$(count)"
run outside tickets list
if [ "$RC" -eq 0 ] && seen="$(check_creds "$before" "$OWNER_TOKEN" "$PASSPORT" 2>&1)"; then
    ok "outside a session: the owner token ($seen)"
else
    bad "outside a session: rc=$RC ${seen:-} err=$(cat "$WORK/err")"
fi

echo ""
echo "tickets_session_passport: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
