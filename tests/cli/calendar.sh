#!/usr/bin/env bash
# k2 calendar / k2 cal (calendars S3: agent calendar CLI) — no daemon, no
# Stalwart. Static wiring + help + schema + study + usage errors, then a
# local echo server checks the exact requests (routes, the --from/--to →
# start/end mapping, POST bodies) and the output/exit contract.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2="$PROJECT_ROOT/cli/k2"
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
            "'send' level" 'hosted_only' 'k2 hostmail calendar enable' \
            '/cli/mail/calendar/' 'BEGIN/END CALENDAR NOTES' 'SHARED CALENDARS' \
            '<owner>/<id>' 'share_rights' 'editor share'; do
  printf '%s' "$help" | grep -qF -- "$want" || fail "calendar --help must mention '$want'"
done
[ "$("$K2" cal --help)" = "$help" ] || fail "k2 cal --help must equal k2 calendar --help"
[ "$("$K2" calendar events --help)" = "$help" ] || fail "verb --help prints the page"
"$K2" help calendar | grep -q 'k2 calendar' || fail "k2 help calendar"
"$K2" mail access --help | grep -q 'k2 calendar' || fail "mail access --help must say grants cover calendars"
"$K2" study mail | grep -q 'k2 calendar' || fail "study mail must list k2 calendar"
"$K2" study mail | grep -q 'read/draft mail grants now also cover calendars' \
  || fail "study mail must say grants expand (CAL32)"

# Schema.
schema="$("$K2" --schema)" || fail "k2 --schema exited non-zero"
for n in list events show freebusy wait create update delete; do
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

out="$(k2 calendar list)"; printf '%s' "$out" | grep -q 'c1 · Main · default' || fail "list output: $out"
out="$(k2 cal events ops@example.com --from 2026-10-08 --to 2026-10-15 --calendar Main --limit 5)"
printf '%s' "$out" | grep -q 'ev_abc · 2026-10-08T16:00:00Z → 2026-10-08T16:30:00Z \[America/Los_Angeles\] · Prep call' \
  || fail "events output: $out"
out="$(k2 calendar show ev_abc)"
printf '%s' "$out" | grep -q 'BEGIN CALENDAR NOTES' || fail "show must wrap notes: $out"
k2 calendar freebusy --from 2026-10-08T00:00:00Z --to 2026-10-09T00:00:00Z | grep -q '16:00:00Z → .* · busy' \
  || fail "freebusy output"
out="$(k2 calendar wait --since-state s1 --timeout 30)"
printf '%s' "$out" | grep -q 'created · ev_new' || fail "wait output: $out"
printf '%s' "$out" | grep -q 'since-state s9' || fail "wait must print the next state: $out"
out="$(k2 calendar create --title "Prep call" --start 2026-10-08T09:00:00-07:00 \
        --end 2026-10-08T09:30:00-07:00 --tz America/Los_Angeles --location "Room 2" --notes "n")"
printf '%s' "$out" | grep -q 'no email was sent' || fail "create output: $out"
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
printf '%s' "$out" | grep -q '^c1 · Main · default · read/write/delete$' || fail "own list row: $out"
printf '%s' "$out" | grep -q '^boss@example.com/c9 · Team · shared by boss@example.com · read/write$' \
  || fail "shared list row: $out"
set_reply /cli/mail/calendar/list 400 '{"ok":false,"error":{"code":"hosted_only","hint":"linked"}}'
set +e; k2 calendar list me@example.com >/dev/null 2>&1; rc=$?; set -e
[ "$rc" -eq 2 ] || fail "hosted_only must exit 2, got $rc"
set_reply /cli/mail/calendar/list 502 '{"ok":false,"error":{"code":"engine","hint":"down"}}'
set +e; k2 calendar list >/dev/null 2>&1; rc=$?; set -e
[ "$rc" -eq 1 ] || fail "engine must exit 1, got $rc"

echo "OK: k2 calendar CLI wiring"
