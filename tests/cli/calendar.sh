#!/usr/bin/env bash
# k2 calendar / k2 cal (calendars S3: agent calendar CLI) — no daemon, no
# Stalwart. Static wiring + help + schema + study + usage errors, then a
# local echo server checks the exact requests (routes, the --from/--to →
# start/end mapping, POST bodies) and the output/exit contract.
# bash-only: reads cli/k2 as source; the parity gate skips it for the binary.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
. "$SCRIPT_DIR/_cli_under_test.sh"; K2="$CLI_BASH_SOURCE"
source "$SCRIPT_DIR/_hermetic_cli.sh"
hermetic_cli_env

fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$K2" ] || fail "$K2 not found"
bash -n "$K2" || fail "bash -n cli/k2"

# Static wiring: router arm + the SECURITY tool-id mapping (CAL23.4).
grep -q '^    calendar|cal)$' "$K2" || fail "top-level router must have a calendar|cal arm"
grep -q 'calendar|cal) echo "mail" ;;' "$K2" || fail "calendar|cal must map to the mail tool id"
eval "$(sed -n '/^# BEGIN_CLI_TOOL_POLICY/,/^# END_CLI_TOOL_POLICY/p' "$K2")"
[ "$(_cli_tool_id_for_verb calendar)" = "mail" ] || fail "calendar tool id"
[ "$(_cli_tool_id_for_verb cal)" = "mail" ] || fail "cal tool id"

# Help.
help="$("$K2" calendar --help)"
for want in 'k2 calendar list' 'events' 'freebusy' 'wait' 'create' 'update' 'delete' \
            'existing read' 'draft mail grants now also cover' 'NEVER any email' \
            "'send' level" 'capability_missing' 'k2 hostmail calendar enable' \
            'LINKED INBOXES' 'Microsoft 365' 'Google Calendar API' 'sendUpdates=none' \
            'k2 mail link calendars on' 'scope_not_granted' \
            '/cli/mail/calendar/' 'BEGIN/END CALENDAR NOTES' 'SHARED CALENDARS' \
            '<owner>/<id>' 'share_rights' 'editor share' \
            'READ-ONLY CALENDAR LINKS' 'k2 cal link --request' 'read_only_link' 'self_response' \
            'needsAction' 'meeting_id' 'never opens it' '--omit-description' 'k2 cal calendars' \
            '1 day back and 45 days ahead' 'k2 cal event <id>'; do
  <<<"$help" grep -qF -- "$want" || fail "calendar --help must mention '$want'"
done
<<<"$help" grep -qF -- 'not_built' && fail "calendar --help must not say Google calendars are not built (MCS4)"
"$K2" mail link --help | grep -qF -- '--features calendar' || fail "mail link --help must teach calendar-only Google links"
[ "$("$K2" cal --help)" = "$help" ] || fail "k2 cal --help must equal k2 calendar --help"
[ "$("$K2" calendar events --help)" = "$help" ] || fail "verb --help prints the page"
"$K2" help calendar | grep -q 'k2 calendar' || fail "k2 help calendar"
"$K2" mail access --help | grep -q 'k2 calendar' || fail "mail access --help must say grants cover calendars"
"$K2" study mail | grep -q 'k2 calendar' || fail "study mail must list k2 calendar"
"$K2" study mail | grep -q 'read/draft mail grants now also cover calendars' \
  || fail "study mail must say grants expand (CAL32)"

# Schema.
schema="$("$K2" --schema)" || fail "k2 --schema exited non-zero"
for n in list events show freebusy wait create update delete calendars link; do
  grep -qF "\"name\": \"calendar $n\"" <<<"$schema" || fail "schema missing calendar $n"
done

# Usage errors exit 2 before any request (K2_PORT=1 is a dead port).
expect_rc() {
  local want="$1"; shift
  set +e
  out="$("$K2" "$@" 2>&1)"
  rc=$?
  set -e
  [ "$rc" -eq "$want" ] || fail "'k2 $*' must exit $want, got $rc: $out"
}
expect_rc 2 calendar wipe
expect_rc 2 calendar show
expect_rc 2 calendar update
expect_rc 2 calendar delete
expect_rc 2 calendar update ev_x
expect_rc 2 calendar create --title t
expect_rc 2 calendar create --start 2026-10-08T09:00:00Z --end 2026-10-08T10:00:00Z
expect_rc 2 calendar events --since-state s
expect_rc 2 calendar list a@example.com b@example.com
expect_rc 2 calendar events --limit x
expect_rc 2 calendar wait --timeout 901
expect_rc 2 calendar events --from
expect_rc 2 cal list --inbox a@example.com
# Calendar S1 usage errors (before any request).
expect_rc 2 calendar link
expect_rc 2 calendar link a@example.com
expect_rc 2 calendar link a@example.com --provider yahoo --workspace w
expect_rc 2 calendar link a@example.com --provider google
expect_rc 2 calendar link a@example.com --provider microsoft --workspace w --paste
expect_rc 2 calendar link a@example.com --provider microsoft --workspace w --paste-file /tmp/x
expect_rc 2 calendar link --request a@example.com --provider google --workspace w
expect_rc 2 calendar link --request a@example.com --provider google --omit-description
expect_rc 2 calendar calendars
expect_rc 2 calendar calendars a@example.com --calendar x
expect_rc 2 calendar show --full
expect_rc 2 mail access grant a@example.com w --calendar-omit-description maybe
expect_rc 2 mail access revoke a@example.com w --calendar-omit-description on

# Live: a local echo server records method, path, query and body, and
# answers from a reply map (path → [status, body]) re-read per request.
WORK="$(mktemp -d -t k2-calendar-XXXXXX)"
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
log_path, replies_path = sys.argv[1], sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def _reply(self, body):
        u = urllib.parse.urlparse(self.path)
        q = {k: v[0] for k, v in urllib.parse.parse_qs(u.query).items()}
        with open(log_path, "a") as log:
            log.write(json.dumps({"m": self.command, "p": u.path, "q": q, "b": body}) + "\n")
        replies = json.load(open(replies_path))
        status, out = replies.get(u.path, [404, {"ok": False, "error": {"code": "not_found", "hint": "no"}}])
        data = json.dumps(out).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        self._reply(None)
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        self._reply(json.loads(self.rfile.read(n).decode() or "null"))
    def log_message(self, *a):
        pass
s = http.server.HTTPServer(("127.0.0.1", 0), H)
print(s.server_address[1], flush=True)
s.serve_forever()
PY

EV='{"id":"ev_abc","title":"Prep call","start":"2026-10-08T16:00:00Z","end":"2026-10-08T16:30:00Z","timeZone":"America/Los_Angeles","allDay":false,"recurring":false,"participants":0,"status":"confirmed"}'
cat >"$WORK/replies.json" <<JSON
{
 "/cli/mail/calendar/list": [200, {"ok":true,"address":"ops@example.com","calendars":[{"id":"c1","name":"Main","isDefault":true}]}],
 "/cli/mail/calendar/events": [200, {"ok":true,"address":"ops@example.com","start":"2026-10-08T00:00:00Z","end":"2026-10-15T00:00:00Z","events":[$EV],"truncated":false}],
 "/cli/mail/calendar/show": [200, {"ok":true,"address":"ops@example.com","event":{"id":"ev_abc","title":"Prep call","start":"2026-10-08T16:00:00Z","end":"2026-10-08T16:30:00Z","description":"bring notes"}}],
 "/cli/mail/calendar/freebusy": [200, {"ok":true,"address":"ops@example.com","start":"2026-10-08T00:00:00Z","end":"2026-10-09T00:00:00Z","busy":[{"start":"2026-10-08T16:00:00Z","end":"2026-10-08T16:30:00Z","status":"busy"}],"source":"availability"}],
 "/cli/mail/calendar/wait": [200, {"ok":true,"timedOut":false,"state":"s9","created":["ev_new"],"updated":[],"destroyed":[]}],
 "/cli/mail/calendar/create": [200, {"ok":true,"address":"ops@example.com","id":"ev_new","event":$EV,"emailSent":false}],
 "/cli/mail/calendar/update": [200, {"ok":true,"id":"ev_abc","event":$EV,"emailSent":false}],
 "/cli/mail/calendar/delete": [200, {"ok":true,"id":"ev_abc","deleted":true,"emailSent":false}]
}
JSON
: >"$WORK/port"
: >"$WORK/requests"
python3 "$WORK/srv.py" "$WORK/requests" "$WORK/replies.json" >"$WORK/port" &
SRV_PID=$!
for _ in $(seq 1 50); do [ -s "$WORK/port" ] && break; sleep 0.1; done
[ -s "$WORK/port" ] || fail "echo server never reported its port"
PORT="$(cat "$WORK/port")"

k2() {
  env -i PATH="$PATH" HOME="$K2_TEST_HOME" K2_PORT="$PORT" K2_HOOK_TOKEN=test-token \
    K2_PROJECT_PATH=/tmp/ws-calendar-test "$K2" "$@"
}

out="$(k2 calendar list)"; <<<"$out" grep -q 'c1 · Main · default' || fail "list output: $out"
out="$(k2 cal events ops@example.com --from 2026-10-08 --to 2026-10-15 --calendar Main --limit 5)"
<<<"$out" grep -q 'ev_abc · 2026-10-08T16:00:00Z → 2026-10-08T16:30:00Z \[America/Los_Angeles\] · Prep call' \
  || fail "events output: $out"
out="$(k2 calendar show ev_abc)"
<<<"$out" grep -q 'BEGIN CALENDAR NOTES' || fail "show must wrap notes: $out"
k2 calendar freebusy --from 2026-10-08T00:00:00Z --to 2026-10-09T00:00:00Z | grep -q '16:00:00Z → .* · busy' \
  || fail "freebusy output"
out="$(k2 calendar wait --since-state s1 --timeout 30)"
<<<"$out" grep -q 'created · ev_new' || fail "wait output: $out"
<<<"$out" grep -q 'since-state s9' || fail "wait must print the next state: $out"
out="$(k2 calendar create --title "Prep call" --start 2026-10-08T09:00:00-07:00 \
        --end 2026-10-08T09:30:00-07:00 --tz America/Los_Angeles --location "Room 2" --notes "n")"
<<<"$out" grep -q 'no email was sent' || fail "create output: $out"
k2 calendar update ev_abc --location "" --title "New" --json | python3 -c 'import json,sys; assert json.load(sys.stdin)["ok"]'
k2 calendar delete ev_abc | grep -q 'deleted ev_abc' || fail "delete output"

cat >"$WORK/check.py" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]).read().splitlines()]
want = [
    ("GET", "/cli/mail/calendar/list", {}, None),
    ("GET", "/cli/mail/calendar/events",
     {"address": "ops@example.com", "start": "2026-10-08", "end": "2026-10-15",
      "calendar": "Main", "limit": "5"}, None),
    ("GET", "/cli/mail/calendar/show", {"id": "ev_abc"}, None),
    ("GET", "/cli/mail/calendar/freebusy",
     {"start": "2026-10-08T00:00:00Z", "end": "2026-10-09T00:00:00Z"}, None),
    ("GET", "/cli/mail/calendar/wait", {"since_state": "s1", "timeout": "30"}, None),
    ("POST", "/cli/mail/calendar/create", {}, {
        "project": "/tmp/ws-calendar-test", "title": "Prep call",
        "start": "2026-10-08T09:00:00-07:00", "end": "2026-10-08T09:30:00-07:00",
        "timeZone": "America/Los_Angeles", "location": "Room 2", "notes": "n"}),
    ("POST", "/cli/mail/calendar/update", {}, {
        "project": "/tmp/ws-calendar-test", "id": "ev_abc", "title": "New", "location": ""}),
    ("POST", "/cli/mail/calendar/delete", {}, {"project": "/tmp/ws-calendar-test", "id": "ev_abc"}),
]
if len(rows) != len(want):
    sys.exit("FAIL: expected %d requests, got %d: %r" % (len(want), len(rows), rows))
for r, (m, p, q, b) in zip(rows, want):
    if (r["m"], r["p"]) != (m, p):
        sys.exit("FAIL: expected %s %s, got %s %s" % (m, p, r["m"], r["p"]))
    got_q = {k: v for k, v in r["q"].items() if k not in ("token", "project")}
    if r["m"] == "GET":
        if r["q"].get("project") != "/tmp/ws-calendar-test":
            sys.exit("FAIL: %s must carry project: %r" % (p, r["q"]))
        # CAL18: the window rides start/end — never from/to.
        if "from" in r["q"] or "to" in r["q"]:
            sys.exit("FAIL: %s must not send from/to: %r" % (p, r["q"]))
        if got_q != q:
            sys.exit("FAIL: %s query must be %r, got %r" % (p, q, got_q))
    if b is not None and r["b"] != b:
        sys.exit("FAIL: %s body must be %r, got %r" % (p, b, r["b"]))
    if r["b"] and "accountId" in r["b"]:
        sys.exit("FAIL: the CLI never sends an accountId")
PY
python3 "$WORK/check.py" "$WORK/requests"

# Calendar S1: event alias, --full / --include-cancelled, calendars, link --request.
: >"$WORK/requests"
set_reply0() {
  python3 - "$WORK/replies.json" "$1" "$2" <<'PY'
import json, sys
p, path, body = sys.argv[1], sys.argv[2], json.loads(sys.argv[3])
d = json.load(open(p)); d[path] = [200, body]; json.dump(d, open(p, "w"))
PY
}
set_reply0 /cli/mail/calendar/show '{"ok":true,"address":"ops@example.com","event":{"id":"ev_abc","title":"Consult","start":"2026-10-08T16:00:00Z","end":"2026-10-08T16:30:00Z","description":null,"redacted":["description"],"join":{"provider":"zoom","meeting_id":"81234567890","url":"https://us02web.zoom.us/j/81234567890"},"organizer":"person@example.com","organizer_is_self":true,"self_response":"accepted","attendees":[{"email":"client@example.net","response":"declined"}]}}'
set_reply0 /cli/mail/calendar/calendars '{"ok":true,"address":"ops@example.com","access":"read-only","calendars":[{"id":"ops@example.com","name":"ops","isDefault":true,"optedIn":true,"primary":true},{"id":"team@example.com","name":"Team","optedIn":true}],"added":["team@example.com"],"removed":[]}'
set_reply0 /cli/mail/calendar/link-request '{"ok":true,"address":"ops@example.com","posted":true,"hint":"a card is on your Thread"}'
out="$(k2 cal event ev_abc)"
<<<"$out" grep -q 'join     : Zoom 81234567890' || fail "event shows the join: $out"
<<<"$out" grep -q 'attendee : client@example.net (declined)' || fail "event lists normalised attendees: $out"
<<<"$out" grep -q 'left out' || fail "event says descriptions were left out: $out"
<<<"$out" grep -q 'BEGIN CALENDAR NOTES' && fail "no notes block when the description is left out: $out"
k2 cal events ops@example.com --full --include-cancelled --json >/dev/null
out="$(k2 cal calendars ops@example.com --add Team --add "Big Room" --remove old)"
<<<"$out" grep -q '^on  team@example.com · Team' || fail "calendars output: $out"
k2 cal calendars ops@example.com >/dev/null
k2 cal link --request ops@example.com --provider microsoft | grep -q 'card' || fail "link --request output"
python3 - "$WORK/requests" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]).read().splitlines()]
got = [(r["m"], r["p"]) for r in rows]
want = [("GET", "/cli/mail/calendar/show"), ("GET", "/cli/mail/calendar/events"),
        ("POST", "/cli/mail/calendar/calendars"), ("GET", "/cli/mail/calendar/calendars"),
        ("POST", "/cli/mail/calendar/link-request")]
assert got == want, got
assert rows[0]["q"]["id"] == "ev_abc", rows[0]
assert rows[1]["q"].get("full") == "1" and rows[1]["q"].get("includeCancelled") == "1", rows[1]
assert rows[2]["b"] == {"address": "ops@example.com", "add": ["Team", "Big Room"], "remove": ["old"]}, rows[2]
assert rows[3]["q"].get("address") == "ops@example.com", rows[3]
assert rows[4]["b"] == {"project": "/tmp/ws-calendar-test", "address": "ops@example.com", "provider": "microsoft"}, rows[4]
PY

# Exit contract: wait timeout → exit 2, nothing on stdout, state in the hint.
set_reply() {
  python3 - "$WORK/replies.json" "$1" "$2" "$3" <<'PY'
import json, sys
p, path, status, body = sys.argv[1], sys.argv[2], int(sys.argv[3]), json.loads(sys.argv[4])
d = json.load(open(p)); d[path] = [status, body]; json.dump(d, open(p, "w"))
PY
}
set_reply /cli/mail/calendar/wait 200 '{"ok":true,"timedOut":true,"state":"s9"}'
set +e
out="$(k2 calendar wait --timeout 5 2>"$WORK/err")"; rc=$?
set -e
[ "$rc" -eq 2 ] || fail "wait timeout must exit 2, got $rc"
[ -z "$out" ] || fail "wait timeout must print nothing on stdout: $out"
grep -q '"code":"timeout"' "$WORK/err" || fail "wait timeout code: $(cat "$WORK/err")"
grep -q 'since-state s9' "$WORK/err" || fail "wait timeout hint must carry the state"

# Needs-your-human codes exit 3; hosted_only exits 2; engine errors exit 1.
for code in invites_need_send_level calendars_disabled email_alerts_need_send_level owner_only share_rights; do
  set_reply /cli/mail/calendar/delete 409 "{\"ok\":false,\"error\":{\"code\":\"$code\",\"hint\":\"h\"}}"
  set +e; k2 calendar delete ev_abc >/dev/null 2>"$WORK/err"; rc=$?; set -e
  [ "$rc" -eq 3 ] || fail "$code must exit 3, got $rc"
  grep -q "\"code\":\"$code\"" "$WORK/err" || fail "$code must be JSON on stderr"
done
# S4: a calendar shared to the inbox shows its owner and rights.
set_reply /cli/mail/calendar/list 200 '{"ok":true,"address":"ops@example.com","calendars":[{"id":"c1","name":"Main","isDefault":true,"owner":"ops@example.com","shared":false,"rights":{"freeBusy":true,"read":true,"write":true,"delete":true}},{"id":"boss@example.com/c9","name":"Team","isDefault":false,"owner":"boss@example.com","shared":true,"rights":{"freeBusy":true,"read":true,"write":true,"delete":false}}]}'
out="$(k2 calendar list)"
<<<"$out" grep -q '^c1 · Main · default · read/write/delete$' || fail "own list row: $out"
<<<"$out" grep -q '^boss@example.com/c9 · Team · shared by boss@example.com · read/write$' \
  || fail "shared list row: $out"
# MCS3: capability_missing exits 3 and keeps {cap, reason, fix} on stderr.
set_reply /cli/mail/calendar/events 409 '{"ok":false,"error":{"code":"capability_missing","hint":"linked for mail only","cap":"calendar.read","reason":"scope_not_granted","fix":"k2 mail link upgrade me@example.com --features calendar (owner)"}}'
set +e; k2 calendar events me@example.com >/dev/null 2>"$WORK/err"; rc=$?; set -e
[ "$rc" -eq 3 ] || fail "capability_missing must exit 3, got $rc"
python3 - "$WORK/err" <<'PY' || fail "capability_missing stderr: $(cat "$WORK/err")"
import json, sys
e = json.loads(open(sys.argv[1]).read())["error"]
assert e["code"] == "capability_missing", e
assert e["cap"] == "calendar.read", e
assert e["reason"] == "scope_not_granted", e
assert e["fix"] == "k2 mail link upgrade me@example.com --features calendar (owner)", e
PY
set_reply /cli/mail/calendar/list 400 '{"ok":false,"error":{"code":"hosted_only","hint":"linked"}}'
set +e; k2 calendar list me@example.com >/dev/null 2>&1; rc=$?; set -e
[ "$rc" -eq 2 ] || fail "hosted_only must exit 2, got $rc"
set_reply /cli/mail/calendar/list 502 '{"ok":false,"error":{"code":"engine","hint":"down"}}'
set +e; k2 calendar list >/dev/null 2>&1; rc=$?; set -e
[ "$rc" -eq 1 ] || fail "engine must exit 1, got $rc"

echo "OK: k2 calendar CLI wiring"
