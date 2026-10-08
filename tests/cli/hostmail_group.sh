#!/usr/bin/env bash
# hostmail group + calendar sharing (calendars S4) CLI wiring — no daemon,
# no Stalwart. Static wiring + help + schema + usage errors, then a local
# echo server checks the exact requests (method, path, body) and the human
# output.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

grep -q 'group)   cmd_hostmail_group' "$K2" || fail "cmd_hostmail must route group"
grep -q 'create|rename|share|unshare|shares|sharing) cmd_hostmail_calendar_share' "$K2" \
  || fail "cmd_hostmail_calendar must route the S4 verbs"

# Help.
ghelp="$("$K2" hostmail group --help)"
for want in 'group create' 'group add' 'group remove' 'group list' 'group delete' \
            'Shared Folders' 'k2 hostmail list' 'mail fan-out' '--mail' \
            "can't widen its own access" 'TEAM CALENDAR' '--with team --write' '--editor'; do
  printf '%s' "$ghelp" | grep -qF -- "$want" || fail "group --help must mention '$want'"
done
chelp="$("$K2" hostmail calendar --help)"
for want in 'calendar create' 'calendar rename' 'calendar share' 'calendar unshare' \
            'calendar shares' 'calendar sharing' 'TEAM CALENDAR' 'k2 hostmail group'; do
  printf '%s' "$chelp" | grep -qF -- "$want" || fail "calendar --help must mention '$want'"
done
shelp="$("$K2" hostmail calendar share --help)"
for want in 'mayReadFreeBusy' 'mayReadItems' 'mayWriteAll' 'mayRSVP' 'mayDelete' 'mayShare' \
            'k2 hostmail upgrade' 'maxShares' '--max-shares' 'k2 calendar'; do
  printf '%s' "$shelp" | grep -qF -- "$want" || fail "calendar share --help must mention '$want'"
done
"$K2" hostmail --help | grep -q 'group create|add|remove|list|show|delete' \
  || fail "hostmail --help must list group"
"$K2" hostmail --help | grep -q 'calendar create|rename|share|unshare|shares|sharing' \
  || fail "hostmail --help must list the calendar sharing verbs"
"$K2" study mail | grep -q 'k2 hostmail group create' || fail "study mail must teach groups"
"$K2" study mail | grep -q 'Team calendar' || fail "study mail must carry the team-calendar recipe"

# Schema.
schema="$("$K2" --schema)" || fail "k2 --schema exited non-zero"
for n in "hostmail calendar create" "hostmail calendar rename" "hostmail calendar share" \
         "hostmail calendar unshare" "hostmail calendar shares" "hostmail calendar sharing" \
         "hostmail group create" "hostmail group add" "hostmail group remove" \
         "hostmail group list" "hostmail group show" "hostmail group delete"; do
  grep -qF "\"name\": \"$n\"" <<<"$schema" || fail "schema missing $n"
done

# Usage errors exit 2 before any request.
expect_usage() {
  set +e
  out="$("$K2" "$@" 2>&1)"
  rc=$?
  set -e
  [ "$rc" -eq 2 ] || fail "'k2 $*' must exit 2, got $rc: $out"
}
expect_usage hostmail group wipe
expect_usage hostmail group create
expect_usage hostmail group create a b
expect_usage hostmail group add team
expect_usage hostmail group add team a@example.com --mail
expect_usage hostmail group list extra
expect_usage hostmail group delete
expect_usage hostmail calendar create owner@example.com
expect_usage hostmail calendar rename owner@example.com Team
expect_usage hostmail calendar share owner@example.com --read
expect_usage hostmail calendar share owner@example.com --with team
expect_usage hostmail calendar share owner@example.com --with team --read --write
expect_usage hostmail calendar share owner@example.com --with team --admin
expect_usage hostmail calendar unshare owner@example.com
expect_usage hostmail calendar unshare owner@example.com --with team --read
expect_usage hostmail calendar shares
expect_usage hostmail calendar sharing --max-shares lots
expect_usage hostmail calendar create owner@example.com Team --with x

# Live: a local echo server records method, path, query and body.
WORK="$(mktemp -d -t k2-hostmail-group-XXXXXX)"
SRV_PID=""
cleanup() {
  if [ -n "$SRV_PID" ]; then
    kill "$SRV_PID" 2>/dev/null || true
    wait "$SRV_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
  hermetic_cli_cleanup
}
trap cleanup EXIT

cat >"$WORK/srv.py" <<'PY'
import http.server, json, sys, urllib.parse
log = open(sys.argv[1], "a")
def reply_for(path, body):
    if path == "/cli/mail/group" and body:
        return {"ok": True, "id": "g1", "address": "team@example.com", "receivesMail": False, "members": []}
    if path == "/cli/mail/group":
        return {"ok": True, "groups": [{"id": "g1", "address": "team@example.com", "name": "team",
                "members": ["a@example.com"], "receivesMail": False}],
                "group": {"id": "g1", "address": "team@example.com", "name": "team",
                "members": ["a@example.com"], "receivesMail": False}}
    if path == "/cli/mail/group/members":
        return {"ok": True, "group": "team@example.com", "members": ["a@example.com", "b@example.com"], "changed": 2}
    if path == "/cli/mail/group/delete":
        return {"ok": True, "group": "team@example.com", "deleted": True, "sharesRemoved": 1, "membersRemoved": 2}
    if path == "/cli/mail/calendar/manage":
        b = json.loads(body)
        if b.get("action") == "create":
            return {"ok": True, "owner": b["owner"], "calendar": {"id": "c1", "name": b["name"]}, "created": True}
        return {"ok": True, "owner": b["owner"], "calendar": {"id": "c1", "name": b["name"], "previousName": "Team"}, "renamed": True}
    if path == "/cli/mail/calendar/share":
        return {"ok": True, "owner": "owner@example.com", "calendar": {"id": "c1", "name": "Team"},
                "with": "team@example.com", "withType": "group", "level": "write", "maxShares": 10}
    if path == "/cli/mail/calendar/unshare":
        return {"ok": True, "owner": "owner@example.com", "calendar": {"id": "c1", "name": "Team"},
                "with": "team@example.com", "removed": True}
    if path == "/cli/mail/calendar/shares":
        return {"ok": True, "owner": "owner@example.com", "maxShares": 10, "calendars": [
            {"id": "c1", "name": "Team", "isDefault": False,
             "shares": [{"with": "team@example.com", "withType": "group", "level": "write"}]}]}
    if path == "/cli/mail/calendar/sharing":
        return {"ok": True, "maxShares": 25, "allowDirectoryQueries": False, "reloaded": True}
    return {"ok": False, "error": {"code": "not_found", "hint": path}}
class H(http.server.BaseHTTPRequestHandler):
    def _reply(self, body):
        u = urllib.parse.urlparse(self.path)
        q = {k: v for k, v in urllib.parse.parse_qsl(u.query) if k != "token"}
        log.write("%s %s %s %s\n" % (self.command, u.path, json.dumps(q, sort_keys=True, separators=(",", ":")), body or "-"))
        log.flush()
        out = json.dumps(reply_for(u.path, body)).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def do_GET(self):
        self._reply("")
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._reply(self.rfile.read(n).decode())
    def log_message(self, *a):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
PY
: >"$WORK/port"
: >"$WORK/requests"
python3 "$WORK/srv.py" "$WORK/requests" >"$WORK/port" &
SRV_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done
[ -s "$WORK/port" ] || fail "echo server never reported its port"
PORT="$(cat "$WORK/port")"

k2() {
  env -i PATH="$PATH" HOME="$K2_TEST_HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token \
    "$K2" "$@"
}

out="$(k2 hostmail group create team)"
printf '%s' "$out" | grep -q 'created group team@example.com (inbound mail off' || fail "group create output: $out"
k2 hostmail group create team --domain example.com --mail --json >/dev/null
out="$(k2 hostmail group add team a@example.com b@example.com)"
printf '%s' "$out" | grep -q 'members=a@example.com,b@example.com' || fail "group add output: $out"
k2 hostmail group remove team b@example.com >/dev/null
out="$(k2 hostmail group list)"
printf '%s' "$out" | grep -q 'group team@example.com · 1 member(s) · inbound mail off' || fail "group list: $out"
k2 hostmail group show team --json >/dev/null
out="$(k2 hostmail group delete team)"
printf '%s' "$out" | grep -q '1 calendar share(s) and 2 membership(s) removed' || fail "group delete: $out"
k2 hostmail calendar create owner@example.com "Team" --color '#3366ff' >/dev/null
out="$(k2 hostmail calendar rename owner@example.com Team Crew)"
printf '%s' "$out" | grep -q "renamed calendar 'Team' → 'Crew'" || fail "rename: $out"
out="$(k2 hostmail calendar share owner@example.com --calendar Team --with team --write)"
printf '%s' "$out" | grep -q "shared 'Team' of owner@example.com with team@example.com: write" || fail "share: $out"
k2 hostmail calendar share owner@example.com --with a@example.com --editor >/dev/null
k2 hostmail calendar share owner@example.com --with a@example.com --freebusy >/dev/null
k2 hostmail calendar share owner@example.com --with a@example.com --read >/dev/null
out="$(k2 hostmail calendar unshare owner@example.com --calendar Team --with team)"
printf '%s' "$out" | grep -q "stopped sharing 'Team'" || fail "unshare: $out"
out="$(k2 hostmail calendar shares owner@example.com)"
printf '%s' "$out" | grep -q 'team@example.com (group): write' || fail "shares: $out"
k2 hostmail calendar sharing >/dev/null
out="$(k2 hostmail calendar sharing --max-shares 25)"
printf '%s' "$out" | grep -q 'max shares per calendar : 25' || fail "sharing: $out"

cat >"$WORK/check.py" <<'PY'
import json, sys
rows = []
for l in open(sys.argv[1]).read().splitlines():
    m, p, q, b = l.split(" ", 3)
    rows.append((m, p, json.loads(q), None if b == "-" else json.loads(b)))
want = [
    ("POST", "/cli/mail/group", {}, {"name": "team", "mail": False}),
    ("POST", "/cli/mail/group", {}, {"name": "team", "mail": True, "domain": "example.com"}),
    ("POST", "/cli/mail/group/members", {}, {"group": "team", "add": ["a@example.com", "b@example.com"]}),
    ("POST", "/cli/mail/group/members", {}, {"group": "team", "remove": ["b@example.com"]}),
    ("GET", "/cli/mail/group", {}, None),
    ("GET", "/cli/mail/group", {"group": "team"}, None),
    ("POST", "/cli/mail/group/delete", {}, {"group": "team"}),
    ("POST", "/cli/mail/calendar/manage", {}, {"action": "create", "owner": "owner@example.com", "name": "Team", "color": "#3366ff"}),
    ("POST", "/cli/mail/calendar/manage", {}, {"action": "rename", "owner": "owner@example.com", "name": "Crew", "calendar": "Team"}),
    ("POST", "/cli/mail/calendar/share", {}, {"owner": "owner@example.com", "with": "team", "calendar": "Team", "level": "write"}),
    ("POST", "/cli/mail/calendar/share", {}, {"owner": "owner@example.com", "with": "a@example.com", "level": "editor"}),
    ("POST", "/cli/mail/calendar/share", {}, {"owner": "owner@example.com", "with": "a@example.com", "level": "freebusy"}),
    ("POST", "/cli/mail/calendar/share", {}, {"owner": "owner@example.com", "with": "a@example.com", "level": "read"}),
    ("POST", "/cli/mail/calendar/unshare", {}, {"owner": "owner@example.com", "with": "team", "calendar": "Team"}),
    ("GET", "/cli/mail/calendar/shares", {"owner": "owner@example.com"}, None),
    ("GET", "/cli/mail/calendar/sharing", {}, None),
    ("POST", "/cli/mail/calendar/sharing", {}, {"maxShares": 25}),
]
if len(rows) != len(want):
    sys.exit("FAIL: expected %d requests, got %d: %r" % (len(want), len(rows), rows))
for got, w in zip(rows, want):
    if got != w:
        sys.exit("FAIL: expected %r, got %r" % (w, got))
PY
python3 "$WORK/check.py" "$WORK/requests"

echo "OK: hostmail group + calendar sharing CLI wiring"
