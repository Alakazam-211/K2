#!/usr/bin/env bash
# Mail + calendar surfaces over linked providers (prd-app-mail-calendar-
# surfaces-v1 §8.3), against a REAL headless k2-daemon under a temp HOME and
# the provider fakes (tests/fakes/oauth-providers) — never Google, never
# Microsoft, never the real ~/.k2, the production daemon or the keychain.
#
# Roles: the owner token (owner-ws = the owner token inside the workspace),
# and an agent holding only its session passport on the cell socket (minted
# by a pinned chat whose harness is a shim).
#
#   MCS1  oauth-config (client id + Microsoft tenant), a Microsoft link by
#         device code (scopes per feature, account check, stamped client),
#         account_mismatch, admin_consent_required, capabilities (owner +
#         agent, masked), `link upgrade` in place, a Google link through the
#         client-capture path, unlink revokes at Google.
#   MCS2  Microsoft mail parity over Graph: compose + reply drafts with an
#         attachment (never sent), approval mode queues a linked send and
#         `hostmail approvals approve` hands it to Graph sendMail (with the
#         attachment, saved to Sent), gate `on` replies via createReply →
#         send to the locked sender, a >3 MB attachment is refused before
#         anything is audited, capabilities show send, Microsoft unlink says
#         where to remove consent.
#   MCS3  Microsoft calendars over Graph: the owner's linked-calendars
#         switch (agents exit 3), list / events / show / create / update /
#         delete / freebusy / wait as the agent passport and the owner token,
#         the draft guard (attendee or outside-organizer events refused, no
#         meeting mail ever), getSchedule with the calendarView fallback,
#         capability_missing (owner_disabled, scope_not_granted) only after
#         the access gate and the level gate.
#   MCS4  Google calendars over the Calendar API: a mail + calendar link
#         (consent URL scopes), list (calendarList, primary-only fallback),
#         events (singleEvents window, zones kept), show, create / update /
#         delete with sendUpdates=none and never attendees (nobody added or
#         emailed), the stored-event guard, freeBusy with the events
#         fallback, wait over syncToken (gs_ states), the owner token; a
#         calendar-only link (calendar works, mail verbs exit 3
#         capability_missing, an unticked consent stores nothing, Microsoft
#         refused); a mail-only link upgraded in place with
#         include_granted_scopes.
#
# Build first (the daemon MUST carry the test-only provider override; the
# script refuses a daemon without it, and points the daemon's outbound
# HTTP at a closed proxy so nothing reaches a real provider):
#   cargo build -p k2-provider-fakes
#   cargo build -p k2-daemon --bin k2-daemon --features provider-fakes
# (A `cargo test -p k2-daemon --test …` build can relink target/debug/
# k2-daemon WITHOUT the feature; rebuild with it before running this.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
FAKES="$TARGET/debug/k2-provider-fakes"
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon --features provider-fakes)" >&2; exit 1; }
[ -x "$FAKES" ] || { echo "FAIL: $FAKES not built (cargo build -p k2-provider-fakes)" >&2; exit 1; }
# Never a real provider. Only a `provider-fakes` build reads
# K2_PROVIDER_FAKES_URL (the name is compiled in only with the feature); a
# daemon without it would send the device-code and token calls to the real
# Microsoft and Google endpoints. Refuse before anything starts.
if ! LC_ALL=C grep -a -q "K2_PROVIDER_FAKES_URL" "$DAEMON"; then
    echo "FAIL: $DAEMON was built without --features provider-fakes, so it would call the real Microsoft and Google endpoints. Rebuild: cargo build -p k2-daemon --bin k2-daemon --features provider-fakes" >&2
    exit 1
fi
for tool in python3 sqlite3 curl; do
    command -v "$tool" >/dev/null || { echo "FAIL: $tool is required" >&2; exit 1; }
done

# Session variables reach the production daemon: drop them all.
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

WORK="$(cd "$(mktemp -d /tmp/k2mcs-XXXXXX)" && pwd -P)"
SB="$WORK/home"
mkdir -p "$SB/tmp" "$SB/run" "$SB/shim"
DAEMON_PID=""
FAKE_PID=""
cleanup() {
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    [ -n "$FAKE_PID" ] && { kill "$FAKE_PID" 2>/dev/null; wait "$FAKE_PID" 2>/dev/null; } || true
    sleep 0.3
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null || true
    pkill -9 -f "frpc -c $SB/" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

pass=0
fail() {
    echo "FAIL: $*" >&2
    echo "--- daemon log tail ---" >&2
    tail -40 "$SB/daemon.log" >&2 2>/dev/null || true
    exit 1
}
ok() { pass=$((pass + 1)); echo "  ok  $*"; }
# run <owner|agent> <args…> → OUT, ERR, RC. Never a TTY on stdin.
run() {
    local who="$1"; shift
    local -a envs=(HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT")
    if [ "$who" = "agent" ]; then
        envs+=(K2_HOOK_TOKEN="$PASSPORT" K2_HOOK_SOCK="$SOCK" K2_PROJECT_PATH="$REPO")
    elif [ "$who" = "owner-ws" ]; then
        envs+=(K2_PROJECT_PATH="$REPO")
    fi
    set +e
    OUT="$(cd "$WORK" && env "${envs[@]}" "$K2_CLI" "$@" </dev/null 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}
# Same as run, with stdin from a string.
run_stdin() {
    local input="$1"; shift
    set +e
    OUT="$(cd "$WORK" && printf %s "$input" | env HOME="$SB" K2_HOST=127.0.0.1 K2_PORT="$DPORT" "$K2_CLI" "$@" 2>"$WORK/err")"
    RC=$?
    set -e
    ERR="$(cat "$WORK/err")"
}
want_rc() { [ "$RC" = "$1" ] || fail "$2: exit $RC (want $1)
stdout: $OUT
stderr: $ERR"; }
has() { [[ "$1" == *"$2"* ]] || fail "$3: missing $(printf %q "$2") in:
$1"; }
jok() { python3 -c 'import json,sys
j = json.loads(sys.argv[1])
sys.exit(0 if eval(sys.argv[2]) else 1)' "$1" "$2" || fail "$3: $2 is false for: $1"; }
sql() { sqlite3 -cmd ".timeout 5000" "$DB" "$1"; }
fake_cfg() { curl -sf -X POST -H 'Content-Type: application/json' --data-raw "$1" "$FAKE/_fake/config" >/dev/null || fail "fake config $1"; }
fake_log() { curl -sf "$FAKE/_fake/log"; }

echo "== provider fakes =="
"$FAKES" --port-file "$WORK/fake.port" >"$WORK/fake.log" 2>&1 &
FAKE_PID=$!
for _ in $(seq 1 100); do [ -s "$WORK/fake.port" ] && break; sleep 0.05; done
[ -s "$WORK/fake.port" ] || fail "the fakes never wrote their port"
FAKE="http://127.0.0.1:$(cat "$WORK/fake.port")"
ok "fakes on $FAKE"

echo "== sandbox daemon (provider-fakes build) =="
cat >"$SB/shim/claude" <<EOF
#!/bin/sh
env > "$SB/claude.env"
exec /bin/cat > /dev/null
EOF
chmod +x "$SB/shim/claude"
TMPDIR="$SB/tmp" XDG_RUNTIME_DIR="$SB/run" K2_RUNTIME_DIR= HOME="$SB" K2_AIRGAP=1 \
    K2_PROVIDER_FAKES_URL="$FAKE" K2_TEST_AGENT_SHIM_DIR="$SB/shim" K2SO_WATCHDOG_DISABLED=1 \
    K2_HEARTBEAT_NO_SELF_HEAL=1 K2_SUBSCRIPTION_PROBE=deny \
    HTTPS_PROXY=http://127.0.0.1:9 HTTP_PROXY=http://127.0.0.1:9 ALL_PROXY=http://127.0.0.1:9 \
    https_proxy=http://127.0.0.1:9 http_proxy=http://127.0.0.1:9 all_proxy=http://127.0.0.1:9 \
    NO_PROXY=127.0.0.1,localhost no_proxy=127.0.0.1,localhost \
    "$DAEMON" >"$SB/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 200); do
    [ -s "$SB/.k2/heartbeat.port" ] && [ -s "$SB/.k2/heartbeat.token" ] && break
    sleep 0.1
done
[ -s "$SB/.k2/heartbeat.port" ] || fail "daemon never wrote its port"
DPORT="$(cat "$SB/.k2/heartbeat.port")"
DTOKEN="$(cat "$SB/.k2/heartbeat.token")"
BASE="http://127.0.0.1:$DPORT"
phase=""
for _ in $(seq 1 300); do
    phase="$(curl -s "$BASE/boot-status" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null || true)"
    [ "$phase" = "ready" ] && break
    sleep 0.1
done
[ "$phase" = "ready" ] || fail "daemon never reached ready"
DB=""
for f in "$SB/.k2/k2.db" "$SB/.k2/k2so.db"; do [ -s "$f" ] && DB="$f" && break; done
[ -n "$DB" ] || fail "found the sandbox DB"
[ "$(sql "SELECT COUNT(*) FROM _migrations WHERE name='0188_mail_oauth_v2'")" = "1" ] || fail "migration 0188 applied"
ok "daemon ready on $DPORT, migration 0188 applied"

echo "== workspaces + agent passport =="
REPO="$WORK/repo"
mkdir -p "$REPO"
# A workspace is a git repo (add-from-path answers needsGitInit otherwise).
git -C "$REPO" -c init.defaultBranch=main init -q
reg="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "seedWiki": False, "seedAgentsMd": False}))' "$REPO")" \
    "$BASE/cli/projects/add-from-path?token=$DTOKEN")"
case "$reg" in *"$REPO"*) ok "workspace registered" ;; *) fail "register: $reg" ;; esac
resp="$(curl -s -X POST "$BASE/cli/workspace/ensure-pinned-chat?token=$DTOKEN" \
    -H 'Content-Type: application/json' --data-raw "$(python3 -c 'import json,sys; print(json.dumps({"project": sys.argv[1]}))' "$REPO")")"
case "$resp" in *'"sessionId"'*) ;; *) fail "pinned chat: $resp" ;; esac
# The shim writes its env when the PTY execs it; give a loaded box 20 s.
for _ in $(seq 1 200); do grep -q '^K2_HOOK_TOKEN=' "$SB/claude.env" 2>/dev/null && break; sleep 0.1; done
PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SB/claude.env" | head -1)"
SOCK="$(sed -n 's/^K2_HOOK_SOCK=//p' "$SB/claude.env" | head -1)"
[ -n "$PASSPORT" ] || fail "the pinned chat got no passport (shim env: $( [ -e "$SB/claude.env" ] && grep -c '^K2' "$SB/claude.env" || echo 'never written') K2 lines; pinned: $resp)"
[ -S "$SOCK" ] || fail "the pinned chat's cell socket is missing: '$SOCK'"
ok "agent passport + cell socket"

echo "== MCS1: oauth-config =="
run owner mail oauth-config set --provider microsoft --client-id fake-ms-client --tenant Organizations --json
want_rc 0 "owner sets the Microsoft app + tenant"
jok "$OUT" 'j["tenant"] == "organizations" and j["configured"] and j["source"] == "custom"' "set answer"
run owner mail oauth-config show
want_rc 0 "owner show"
has "$OUT" "microsoft: your own app fake-ms-client" "show names the app"
has "$OUT" "tenant organizations" "show names the tenant"
run agent mail oauth-config show
want_rc 3 "an agent can't read the OAuth config"
has "$ERR" "owner_only" "agent oauth-config → owner_only"
run owner mail oauth-config set --provider microsoft --tenant "not a tenant"
want_rc 2 "a bad tenant is a usage error"
code="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/cli/mail/oauth-config/set?token=$DTOKEN")"
[ "$code" = "405" ] || fail "oauth-config/set refuses GET (got $code)"
ok "oauth-config/set is POST-only (405)"

echo "== MCS1: Microsoft link by device code =="
MS="rosson@contoso.example"
run owner mail link add "$MS" --provider microsoft --workspace "$REPO"
want_rc 0 "link add (Microsoft)"
has "$OUT" "connected $MS" "link connected"
has "$ERR" "FAKE-" "the device code was shown"
ROW="$(sql "SELECT id FROM mail_external_inboxes WHERE email_address='$MS'")"
[ -n "$ROW" ] || fail "the row exists"
[ "$(sql "SELECT features||'|'||oauth_client||'|'||account_kind||'|'||account_tenant FROM mail_external_inboxes WHERE id='$ROW'")" \
    = "mail|byo|work|11111111-2222-3333-4444-555555555555" ] || fail "row stamp: $(sql "SELECT features, oauth_client, account_kind, account_tenant FROM mail_external_inboxes WHERE id='$ROW'")"
[ "$(sql "SELECT granted_scopes LIKE '%Mail.Send%' AND granted_scopes NOT LIKE '%Calendars%' FROM mail_external_inboxes WHERE id='$ROW'")" = "1" ] \
    || fail "granted scopes: $(sql "SELECT granted_scopes FROM mail_external_inboxes WHERE id='$ROW'")"
[ -n "$(sql "SELECT account_checked_at FROM mail_external_inboxes WHERE id='$ROW'")" ] || fail "account_checked_at set"
ok "row stamped: mail, byo client, work account, tenant, Mail.Send granted, checked"
LOG="$(fake_log)"
jok "$LOG" 'any(r["path"] == "/ms/login/organizations/oauth2/v2.0/devicecode" and r["form"].get("client_id") == "fake-ms-client" and "Mail.Send" in r["form"].get("scope", "") and "Calendars" not in r["form"].get("scope", "") for r in j["requests"])' \
    "device start went to the tenant authority with the BYO client and the mail scope set"
ok "device code asked the organizations authority for the mail scope set"
[ -z "$(grep -l 'ms-rt-\|ms-at-' "$SB/.k2/k2.db" 2>/dev/null || true)" ] || fail "a token reached the DB file"
ok "no token in the DB"

echo "== MCS1: capabilities =="
run owner mail capabilities "$MS" --json
want_rc 0 "owner capabilities (one inbox)"
jok "$OUT" 'j["capabilities"]["source"] == "microsoft" and j["capabilities"]["account"]["kind"] == "work" and j["capabilities"]["mail"]["read"] and j["capabilities"]["mail"]["draft"]' "owner sees the microsoft inbox"
jok "$OUT" 'any(m["cap"] == "calendar.read" and m["reason"] == "scope_not_granted" and "link upgrade" in m["fix"] for m in j["capabilities"]["missing"])' "calendar names the upgrade"
ok "owner: source microsoft, work account, calendar → upgrade fix"
run agent mail capabilities --json
want_rc 0 "agent capabilities"
jok "$OUT" 'any(r["address"] == "rosson@contoso.example" and r["yourLevel"] == "draft" for r in j["inboxes"])' "agent sees its primary inbox at draft"
jok "$OUT" 'all(r["capabilities"]["mail"]["send"] is False for r in j["inboxes"] if r["address"] == "rosson@contoso.example")' "draft level can't send"
run agent mail capabilities nobody@elsewhere.example
want_rc 1 "an inbox the agent can't see"
has "$ERR" "not_found" "masked not_found"
ok "agent: own inbox at draft, other inboxes masked"

echo "== MCS1: account check =="
fake_cfg '{"msAccount": "someone.else@contoso.example"}'
run owner mail link add second@contoso.example --provider microsoft --workspace "$REPO"
want_rc 1 "a link signed in as someone else fails"
has "$ERR" "account_mismatch" "code account_mismatch"
has "$ERR" "someone.else@contoso.example" "the hint names who signed in"
[ "$(sql "SELECT COUNT(*) FROM mail_external_inboxes WHERE email_address='second@contoso.example'")" = "0" ] || fail "nothing stored on a mismatch"
ok "account_mismatch stores nothing"

echo "== MCS1: admin consent =="
fake_cfg '{"msAccount": "third@contoso.example", "msTokenError": "AADSTS65001: The user or administrator has not consented to use the application"}'
run owner mail link add third@contoso.example --provider microsoft --workspace "$REPO"
want_rc 1 "a tenant that needs admin consent"
has "$ERR" "admin_consent_required" "code admin_consent_required"
has "$ERR" "/organizations/adminconsent?client_id=fake-ms-client" "the admin-consent URL"
[ "$(sql "SELECT COUNT(*) FROM mail_external_inboxes WHERE email_address='third@contoso.example'")" = "0" ] || fail "nothing stored"
fake_cfg '{"msAccount": "rosson@contoso.example", "msTokenError": ""}'
ok "admin_consent_required carries the admin URL, stores nothing"

echo "== MCS1: link upgrade (in place) =="
run agent mail link upgrade "$MS" --features calendar
want_rc 3 "an agent can't re-consent"
run owner mail link upgrade "$MS" --features calendar
want_rc 0 "owner upgrade"
has "$OUT" "upgraded $MS" "upgrade output"
[ "$(sql "SELECT id FROM mail_external_inboxes WHERE email_address='$MS'")" = "$ROW" ] || fail "same row id"
[ "$(sql "SELECT features FROM mail_external_inboxes WHERE id='$ROW'")" = "mail,calendar" ] || fail "features after upgrade"
LOG="$(fake_log)"
jok "$LOG" 'any(r["path"].endswith("/devicecode") and "Calendars.ReadWrite" in r["form"].get("scope", "") and "Mail.Send" in r["form"].get("scope", "") for r in j["requests"])' "upgrade asked for mail + calendar"
run owner mail capabilities "$MS" --json
jok "$OUT" 'all(m["cap"] not in ("calendar.read", "calendar.write") for m in j["capabilities"]["missing"]) and j["capabilities"]["calendar"]["read"] and j["capabilities"]["calendar"]["write"] and j["capabilities"]["calendar"]["freebusy"] == "self" and all(m["reason"] == "not_built" for m in j["capabilities"]["missing"] if m["cap"] in ("calendar.invite", "calendar.rsvp"))' "calendar read/write on now (Graph); invites not built"
ok "upgrade: same row, mail,calendar consented"

echo "== MCS3: the owner's linked-calendars switch =="
run owner mail link calendars --json
want_rc 0 "owner reads the switch"
jok "$OUT" 'j["enabled"] is True and j["linkedCalendars"] == "on"' "linked calendars default on"
run agent mail link calendars off
want_rc 3 "an agent can't flip the switch"
has "$ERR" "owner_only" "agent link calendars → owner_only"
code="$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' \
    --data-raw '{"enabled":"maybe"}' "$BASE/cli/mail/link/calendars?token=$DTOKEN")"
[ "$code" = "400" ] || fail "a non-boolean switch body is a usage error (got $code)"
run owner mail link calendars sometimes
want_rc 2 "link calendars takes on|off"
ok "switch: owner reads it, agents exit 3, bad input is usage"

echo "== MCS3: Microsoft calendars over Graph (agent passport at draft) =="
run agent calendar list "$MS" --json
want_rc 0 "agent lists the Microsoft calendars"
jok "$OUT" 'j["source"] == "microsoft" and [c["id"] for c in j["calendars"]] == ["cal-default", "cal-shared"]' "both calendars"
jok "$OUT" 'j["calendars"][0]["rights"]["write"] is True and j["calendars"][0]["isDefault"] and j["calendars"][1]["shared"] and j["calendars"][1]["rights"]["write"] is False' "own default writable, shared read-only"
run agent calendar list "$MS"
want_rc 0 "agent lists (text)"
has "$OUT" "cal-shared · Boss · shared by boss@contoso.example" "text list names the sharer"
ok "list: own + shared calendars with rights"

run agent calendar events "$MS" --from 2026-10-19 --to 2026-10-26 --json
want_rc 0 "agent reads the week"
jok "$OUT" '[e["title"] for e in j["events"]] == ["Focus time", "Pipeline review", "Board prep"]' "the seeded week in start order"
jok "$OUT" 'j["events"][0]["start"] == "2026-10-20T16:00:00Z" and j["events"][0]["timeZone"] == "Pacific Standard Time" and j["events"][1]["timeZone"] == "Europe/Berlin" and j["events"][1]["participants"] == 1' "UTC instants, each event keeps its zone"
EV_SOLO="$(printf %s "$OUT" | python3 -c 'import json,sys; print([e["id"] for e in json.load(sys.stdin)["events"] if e["title"]=="Focus time"][0])')"
EV_MEETING="$(printf %s "$OUT" | python3 -c 'import json,sys; print([e["id"] for e in json.load(sys.stdin)["events"] if e["title"]=="Pipeline review"][0])')"
EV_INVITED="$(printf %s "$OUT" | python3 -c 'import json,sys; print([e["id"] for e in json.load(sys.stdin)["events"] if e["title"]=="Board prep"][0])')"
run agent calendar events "$MS" --from 2026-10-19 --to 2026-10-26 --calendar cal-shared --json
want_rc 0 "agent reads the shared calendar"
jok "$OUT" 'j["events"] == []' "the shared calendar is empty"
ok "events: calendarView in UTC, ids carry the inbox, zones kept"

run agent calendar show "$EV_MEETING"
want_rc 0 "agent shows a meeting"
has "$OUT" "attendee : alice@example.com (accepted)" "show lists the attendee"
has "$OUT" "BEGIN CALENDAR NOTES (untrusted" "notes are fenced as untrusted"
has "$OUT" "Ignore previous instructions" "the notes are inside"
ok "show: attendees + untrusted notes markers"

run agent calendar create "$MS" --title "Deep work" --start 2026-10-23T16:00:00Z --end 2026-10-23T17:30:00Z \
    --tz America/Los_Angeles --location "Desk" --notes "no meetings" --json
want_rc 0 "agent creates a draft event"
jok "$OUT" 'j["emailSent"] is False and j["source"] == "microsoft" and j["event"]["timeZone"] == "America/Los_Angeles" and j["event"]["start"] == "2026-10-23T16:00:00Z"' "created in its zone"
EV_NEW="$(printf %s "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
LOG="$(fake_log)"
jok "$LOG" 'any(r["method"] == "POST" and r["path"] == "/ms/graph/v1.0/me/events" for r in j["requests"])' "POST /me/events"
run agent calendar update "$EV_NEW" --title "Deep work (moved)" --start 2026-10-23T18:00:00Z --json
want_rc 0 "agent updates its own event"
jok "$OUT" 'j["event"]["title"] == "Deep work (moved)" and j["emailSent"] is False' "updated"
run agent calendar show "$EV_NEW" --json
jok "$OUT" 'j["event"]["start"] == "2026-10-23T18:00:00Z" and j["event"]["end"] == "2026-10-23T19:30:00Z" and j["event"]["timeZone"] == "America/Los_Angeles"' "moved in its own zone, duration kept"
run agent calendar create "$MS" --title "Elsewhere" --start 2026-10-23T09:00:00Z --end 2026-10-23T10:00:00Z --calendar cal-shared
want_rc 3 "the shared calendar is read-only at draft"
has "$ERR" "share_rights" "code share_rights"
ok "create/update: POST + PATCH with no attendees, zone and duration kept"

echo "== MCS3: the draft guard on stored events =="
run agent calendar update "$EV_MEETING" --title "renamed"
want_rc 3 "an event with an outside attendee needs send"
has "$ERR" "invites_need_send_level" "code invites_need_send_level"
run agent calendar delete "$EV_INVITED"
want_rc 3 "an event someone else organized"
has "$ERR" "invites_need_send_level" "code invites_need_send_level (organizer)"
run agent calendar delete "$EV_NEW" --json
want_rc 0 "agent deletes its own event"
jok "$OUT" 'j["deleted"] is True and j["emailSent"] is False' "deleted"
LOG="$(fake_log)"
jok "$LOG" 'not any(r["method"] in ("PATCH", "DELETE") and r["path"].endswith("/me/events/ev-meeting") for r in j["requests"])' "the meeting was never written"
jok "$LOG" 'not any(r["method"] in ("PATCH", "DELETE") and r["path"].endswith("/me/events/ev-invited") for r in j["requests"])' "the organizer's event was never written"
jok "$LOG" 'any(r["method"] == "DELETE" for r in j["requests"])' "DELETE reached Graph for the agent's own event"
jok "$LOG" 'not any(s["via"] in ("meetingRequest", "meetingUpdate", "meetingCancel", "organizerNotice") for s in j["sent"])' "Exchange was never asked to email anyone"
jok "$LOG" 'all("outlook.timezone=\"UTC\"" in (r.get("prefer") or "") for r in j["requests"] if "/calendar" in r["path"] or "/events" in r["path"])' "every calendar request asked for UTC"
ok "guard: attendee / outside-organizer events refused, nobody emailed, UTC asked"

echo "== MCS3: free/busy =="
run agent calendar freebusy "$MS" --from 2026-10-19T00:00:00Z --to 2026-10-26T00:00:00Z --json
want_rc 0 "agent free/busy"
jok "$OUT" 'j["source"] == "getSchedule" and [b["status"] for b in j["busy"]] == ["busy", "busy", "tentative"]' "getSchedule busy windows"
[[ "$OUT" != *"Board prep"* && "$OUT" != *"Focus time"* ]] || fail "free/busy leaked a title: $OUT"
fake_cfg '{"getScheduleRefused": true}'
run agent calendar freebusy "$MS" --from 2026-10-19T00:00:00Z --to 2026-10-26T00:00:00Z --json
want_rc 0 "free/busy when getSchedule is refused"
jok "$OUT" 'j["source"] == "events" and len(j["busy"]) == 3' "computed from the calendar view"
fake_cfg '{"getScheduleRefused": false}'
ok "free/busy: getSchedule, calendarView fallback, no titles"

echo "== MCS3: wait (calendarView delta) =="
run agent calendar wait "$MS" --timeout 5
want_rc 2 "wait times out with a state"
STATE="$(printf %s "$ERR" | sed -n 's/.*--since-state \(gd_[A-Za-z0-9_-]*\).*/\1/p')"
[ -n "$STATE" ] || fail "the timeout hint carries a gd_ state: $ERR"
run agent calendar create "$MS" --title "Late add" --start 2026-10-24T16:00:00Z --end 2026-10-24T17:00:00Z --json
want_rc 0 "a change to wait for"
EV_LATE="$(printf %s "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
run agent calendar wait "$MS" --since-state "$STATE" --timeout 30 --json
want_rc 0 "wait sees the change"
jok "$OUT" '"'"$EV_LATE"'" in j["updated"] and j["state"].startswith("gd_") and j["source"] == "microsoft"' "the new event is reported"
run agent calendar wait "$MS" --since-state s-not-graph --timeout 5
want_rc 2 "a foreign state is usage"
LOG="$(fake_log)"
jok "$LOG" 'all(r["path"] == "/ms/graph/v1.0/me/calendarView/delta" for r in j["requests"] if "delta" in r["path"])' "delta only ever hit the Graph delta path"
ok "wait: baseline → state → change reported"

echo "== MCS3: capability errors (after the access gate) =="
run owner mail link calendars off
want_rc 0 "owner turns linked calendars off"
has "$OUT" "Calendars on linked inboxes: off (changed)" "switch output"
run agent calendar list "$MS"
want_rc 3 "owner_disabled exits 3"
jok "$ERR" 'j["error"]["code"] == "capability_missing" and j["error"]["reason"] == "owner_disabled" and j["error"]["fix"] == "k2 mail link calendars on (owner)"' "owner_disabled with the fix"
run agent mail capabilities "$MS" --json
jok "$OUT" 'any(m["cap"] == "calendar.read" and m["reason"] == "owner_disabled" for m in j["capabilities"]["missing"])' "capabilities agree"
run owner mail link calendars on
want_rc 0 "owner turns them back on"
run agent calendar list "$MS"
want_rc 0 "back on"
run agent calendar list nobody@elsewhere.example
want_rc 1 "an inbox the agent can't see"
has "$ERR" "not_found" "masked not_found, never capability_missing"
run owner mail access level "$MS" "$REPO" --level read
want_rc 0 "owner lowers the agent to read"
run agent calendar create "$MS" --title "x" --start 2026-10-24T09:00:00Z --end 2026-10-24T10:00:00Z
want_rc 1 "read level can't write"
has "$ERR" "not_found" "a read grant answers the masked not_found"
run agent calendar events "$MS" --from 2026-10-19 --to 2026-10-26
want_rc 0 "read level still reads"
run owner mail access level "$MS" "$REPO" --level draft
want_rc 0 "owner restores draft"
fake_cfg '{"msAccount": "mailonly@contoso.example"}'
run owner mail link add mailonly@contoso.example --provider microsoft --workspace "$REPO"
want_rc 0 "a mail-only Microsoft link"
fake_cfg '{"msAccount": "rosson@contoso.example"}'
run agent calendar events mailonly@contoso.example
want_rc 3 "mail-only link exits 3"
jok "$ERR" 'j["error"]["code"] == "capability_missing" and j["error"]["reason"] == "scope_not_granted" and j["error"]["cap"] == "calendar.read" and j["error"]["fix"] == "k2 mail link upgrade mailonly@contoso.example --features calendar (owner)"' "scope_not_granted with the upgrade fix"
run owner mail link remove mailonly@contoso.example
want_rc 0 "drop the mail-only link"
ok "capability_missing: owner_disabled, scope_not_granted; masked + level gates first"

echo "== MCS3: owner token in the workspace =="
run owner-ws calendar events "$MS" --from 2026-10-19 --to 2026-10-26 --json
want_rc 0 "owner token reads the Microsoft calendar"
jok "$OUT" 'len(j["events"]) == 4' "owner sees the same week (plus the late add)"
run owner-ws calendar delete "$EV_LATE"
want_rc 0 "owner deletes the late add"
ok "owner: same verbs, same answers"

echo "== MCS1: Google link (client capture) + revoke on unlink =="
run_stdin "fake-google-secret" mail oauth-config set --provider gmail --client-id fake-g.apps.googleusercontent.com --secret-stdin
want_rc 0 "owner sets a Gmail app (secret on stdin)"
[[ "$OUT" != *"fake-google-secret"* ]] || fail "the secret was echoed"
G="rosson@gmail.example"
start="$(curl -s -X POST "$BASE/cli/mail/link/oauth/start?token=$DTOKEN" -H 'Content-Type: application/json' \
    --data-raw "{\"address\":\"$G\",\"provider\":\"gmail\",\"workspace\":\"$REPO\",\"clientCapture\":true,\"redirectUri\":\"http://127.0.0.1:9/cb\"}")"
jok "$start" '"login_hint=rosson%40gmail.example" in j["authorizationUrl"] and j["features"] == "mail"' "consent URL has the login hint"
LINK_ID="$(printf %s "$start" | python3 -c 'import json,sys; print(json.load(sys.stdin)["linkId"])')"
STATE="$(printf %s "$start" | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"])')"
done_="$(curl -s -X POST "$BASE/cli/mail/link/oauth/complete?token=$DTOKEN" -H 'Content-Type: application/json' \
    --data-raw "{\"linkId\":\"$LINK_ID\",\"code\":\"fake-code\",\"state\":\"$STATE\"}")"
jok "$done_" 'j["state"] == "connected"' "Google link completes"
[ "$(sql "SELECT provider||'|'||account_kind||'|'||oauth_client FROM mail_external_inboxes WHERE email_address='$G'")" = "gmail|consumer|byo" ] \
    || fail "google row: $(sql "SELECT provider, account_kind, oauth_client FROM mail_external_inboxes WHERE email_address='$G'")"
ok "Google row: consumer account, BYO client"
run owner mail link remove "$G"
want_rc 0 "unlink Google"
has "$OUT" "revoked at Google" "unlink says it revoked"
jok "$(fake_log)" 'j["revoked"] == ["refresh"]' "the refresh token was revoked at Google"
ok "unlink revoked the Google grant first"

echo "== MCS4: Google calendars (a link consented for mail + calendar) =="
# glink <address> <features-json> [granted-scope] → a Google link through the
# client-capture path; the fake grants exactly the consent URL's scope
# unless a narrower one is given. Sets LSTART (start answer) and LDONE.
glink() {
    local addr="$1" feats="$2" grant="${3:-}" body scope
    body="$(python3 -c 'import json,sys; print(json.dumps({"address": sys.argv[1], "provider": "gmail", "workspace": sys.argv[2], "features": json.loads(sys.argv[3]), "clientCapture": True, "redirectUri": "http://127.0.0.1:9/cb"}))' "$addr" "$REPO" "$feats")"
    LSTART="$(curl -s -X POST "$BASE/cli/mail/link/oauth/start?token=$DTOKEN" -H 'Content-Type: application/json' --data-raw "$body")"
    scope="$(printf %s "$LSTART" | python3 -c 'import json,sys,urllib.parse as u; d=json.load(sys.stdin); print(u.parse_qs(u.urlparse(d["authorizationUrl"]).query)["scope"][0])')" \
        || fail "glink $addr: no authorizationUrl in: $LSTART"
    [ -n "$grant" ] || grant="$scope"
    fake_cfg "$(python3 -c 'import json,sys; print(json.dumps({"googleAccount": sys.argv[1], "googleScope": sys.argv[2]}))' "$addr" "$grant")"
    local lid st
    lid="$(printf %s "$LSTART" | python3 -c 'import json,sys; print(json.load(sys.stdin)["linkId"])')"
    st="$(printf %s "$LSTART" | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"])')"
    curl -s -X POST "$BASE/cli/mail/link/oauth/complete?token=$DTOKEN" -H 'Content-Type: application/json' \
        --data-raw "{\"linkId\":\"$lid\",\"code\":\"fake-code\",\"state\":\"$st\"}" >/dev/null
    LDONE="$(curl -s "$BASE/cli/mail/link/oauth/status?token=$DTOKEN&linkId=$lid")"
}
GC="rosson@gmail.example"
glink "$GC" '"mail,calendar"'
jok "$LSTART" 'all(s in j["authorizationUrl"] for s in ("calendar.events", "calendar.calendarlist.readonly", "calendar.freebusy", "mail.google.com")) and j["features"] == "mail,calendar"' "consent URL asks mail + the three calendar scopes"
jok "$LDONE" 'j["state"] == "connected"' "the mail + calendar Google link connects"
GROW="$(sql "SELECT id FROM mail_external_inboxes WHERE email_address='$GC'")"
[ "$(sql "SELECT features FROM mail_external_inboxes WHERE id='$GROW'")" = "mail,calendar" ] || fail "features mail,calendar"
run owner mail capabilities "$GC" --json
want_rc 0 "owner capabilities (Google)"
jok "$OUT" 'j["capabilities"]["source"] == "google" and j["capabilities"]["calendar"]["read"] and j["capabilities"]["calendar"]["write"] and j["capabilities"]["calendar"]["freebusy"] == "self" and all(m["cap"] not in ("calendar.read", "calendar.write") for m in j["capabilities"]["missing"]) and all(m["reason"] != "not_built" or m["cap"] in ("calendar.invite", "calendar.rsvp") for m in j["capabilities"]["missing"])' "Google calendar read/write on; only invites not built"
ok "Google link with calendar consent; capabilities: read/write on, free/busy self"

echo "== MCS4: Google calendars over the Calendar API (agent passport at draft) =="
run agent calendar list "$GC" --json
want_rc 0 "agent lists the Google calendars"
jok "$OUT" 'j["source"] == "google" and [c["id"] for c in j["calendars"]] == ["rosson@gmail.example", "team@group.calendar.google.com", "boss@example.com"]' "calendar list ids"
jok "$OUT" 'j["calendars"][0]["isDefault"] and j["calendars"][0]["rights"]["write"] and j["calendars"][1]["rights"]["write"] and j["calendars"][2]["shared"] and j["calendars"][2]["rights"]["write"] is False' "own calendars writable, the shared one read-only"
ok "list: the account's calendar list with rights"

run agent calendar events "$GC" --from 2026-10-19 --to 2026-10-26 --json
want_rc 0 "agent reads the Google week"
jok "$OUT" '[e["title"] for e in j["events"]] == ["Gym", "Design review", "Investor call", "Lunch (free)"] and j["source"] == "google"' "the seeded week in start order"
jok "$OUT" 'j["events"][0]["start"] == "2026-10-20T15:00:00Z" and j["events"][0]["timeZone"] == "America/Los_Angeles" and j["events"][0]["localStart"] == "2026-10-20T08:00:00" and j["events"][1]["timeZone"] == "Europe/Berlin" and j["events"][1]["start"] == "2026-10-21T13:00:00Z" and j["events"][1]["participants"] == 2 and j["events"][3]["freeBusy"] == "free"' "UTC instants, each event keeps its zone, transparent = free"
gid() { printf %s "$OUT" | python3 -c 'import json,sys; print([e["id"] for e in json.load(sys.stdin)["events"] if e["title"]==sys.argv[1]][0])' "$1"; }
GEV_MEETING="$(gid "Design review")"
GEV_INVITED="$(gid "Investor call")"
LOG="$(fake_log)"
jok "$LOG" 'any(r["path"] == "/google/calendar/v3/calendars/primary/events" and r["query"].get("singleEvents") == "true" and r["query"].get("orderBy") == "startTime" and r["query"].get("timeMin") == "2026-10-19T00:00:00Z" for r in j["requests"])' "events.list singleEvents in the window"
run agent calendar events "$GC" --from 2026-10-19 --to 2026-10-26 --calendar boss@example.com --json
want_rc 0 "agent reads a shared Google calendar"
jok "$OUT" 'j["events"] == []' "the shared calendar is empty"
ok "events: singleEvents window, ids carry the inbox, zones kept"

run agent calendar show "$GEV_MEETING"
want_rc 0 "agent shows the Google meeting"
has "$OUT" "attendee : alice@example.com (accepted)" "show lists the attendee"
has "$OUT" "BEGIN CALENDAR NOTES (untrusted" "notes are fenced as untrusted"
has "$OUT" "Ignore previous instructions" "the notes are inside"
ok "show: attendees + untrusted notes markers"

run agent calendar create "$GC" --title "Deep work" --start 2026-10-23T16:00:00Z --end 2026-10-23T17:30:00Z \
    --tz America/Los_Angeles --location "Desk" --notes "no meetings" --json
want_rc 0 "agent creates a draft Google event"
jok "$OUT" 'j["emailSent"] is False and j["source"] == "google" and j["event"]["timeZone"] == "America/Los_Angeles" and j["event"]["start"] == "2026-10-23T16:00:00Z" and j["event"]["end"] == "2026-10-23T17:30:00Z"' "created in its zone"
GEV_NEW="$(printf %s "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
run agent calendar update "$GEV_NEW" --title "Deep work (moved)" --start 2026-10-23T18:00:00Z --json
want_rc 0 "agent moves its own Google event"
jok "$OUT" 'j["event"]["title"] == "Deep work (moved)" and j["emailSent"] is False' "updated"
run agent calendar show "$GEV_NEW" --json
jok "$OUT" 'j["event"]["start"] == "2026-10-23T18:00:00Z" and j["event"]["end"] == "2026-10-23T19:30:00Z" and j["event"]["timeZone"] == "America/Los_Angeles"' "moved in its own zone, duration kept"
run agent calendar create "$GC" --title "Elsewhere" --start 2026-10-23T09:00:00Z --end 2026-10-23T10:00:00Z --calendar boss@example.com
want_rc 3 "the shared Google calendar is read-only at draft"
has "$ERR" "share_rights" "code share_rights"
run agent calendar create "$GC" --title "Team sync" --start 2026-10-23T09:00:00Z --end 2026-10-23T10:00:00Z \
    --calendar team@group.calendar.google.com --json
want_rc 0 "an owned secondary calendar takes a draft"
GEV_TEAM="$(printf %s "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
run agent calendar delete "$GEV_TEAM" --json
want_rc 0 "agent deletes the secondary-calendar event"
LOG="$(fake_log)"
jok "$LOG" 'any(r["method"] == "POST" and r["path"] == "/google/calendar/v3/calendars/team%40group.calendar.google.com/events" or r["method"] == "POST" and r["path"] == "/google/calendar/v3/calendars/team@group.calendar.google.com/events" for r in j["requests"])' "the secondary calendar got the insert"
ok "create/update/delete: sendUpdates=none, zone and duration kept, shared calendars read-only"

echo "== MCS4: the draft guard on stored Google events =="
run agent calendar update "$GEV_MEETING" --title "renamed"
want_rc 3 "a Google event with an outside attendee needs send"
has "$ERR" "invites_need_send_level" "code invites_need_send_level"
run agent calendar delete "$GEV_INVITED"
want_rc 3 "a Google event someone else organised"
has "$ERR" "invites_need_send_level" "code invites_need_send_level (organizer)"
run agent calendar delete "$GEV_NEW" --json
want_rc 0 "agent deletes its own Google event"
jok "$OUT" 'j["deleted"] is True and j["emailSent"] is False' "deleted"
LOG="$(fake_log)"
jok "$LOG" 'not any(r["method"] in ("PATCH", "DELETE") and r["path"].endswith(("/events/g-meeting", "/events/g-invited")) for r in j["requests"])' "the meeting and the organiser's event were never written"
jok "$LOG" 'all(r["query"].get("sendUpdates") == "none" for r in j["requests"] if r["path"].startswith("/google/calendar/v3/calendars/") and r["method"] in ("POST", "PATCH", "DELETE"))' "every Google write said sendUpdates=none"
jok "$LOG" 'not any(s.get("via", "").startswith("google") for s in j["sent"])' "Google was never asked to add or email anyone"
ok "guard: attendee / outside-organiser events refused, nobody added or emailed"

echo "== MCS4: Google free/busy + fallbacks =="
run agent calendar freebusy "$GC" --from 2026-10-19T00:00:00Z --to 2026-10-26T00:00:00Z --json
want_rc 0 "agent Google free/busy"
jok "$OUT" 'j["source"] == "freeBusy" and len(j["busy"]) == 3 and all(b["status"] == "busy" for b in j["busy"])' "freeBusy windows (the free lunch skipped)"
[[ "$OUT" != *"Gym"* && "$OUT" != *"Investor"* ]] || fail "free/busy leaked a title: $OUT"
fake_cfg '{"googleFreeBusyRefused": true}'
run agent calendar freebusy "$GC" --from 2026-10-19T00:00:00Z --to 2026-10-26T00:00:00Z --json
want_rc 0 "free/busy without the freebusy scope"
jok "$OUT" 'j["source"] == "events" and [b["status"] for b in j["busy"]] == ["busy", "busy", "tentative"]' "computed from the events (tentative kept)"
fake_cfg '{"googleFreeBusyRefused": false, "googleCalendarListRefused": true}'
run agent calendar list "$GC" --json
want_rc 0 "list without the calendar-list scope"
jok "$OUT" '[c["id"] for c in j["calendars"]] == ["rosson@gmail.example"] and j["calendars"][0]["rights"]["write"]' "the primary calendar alone"
fake_cfg '{"googleCalendarListRefused": false}'
ok "free/busy: freeBusy, events fallback; list: primary-only fallback"

echo "== MCS4: wait (syncToken) =="
run agent calendar wait "$GC" --timeout 5
want_rc 2 "Google wait times out with a state"
GSTATE="$(printf %s "$ERR" | sed -n 's/.*--since-state \(gs_[A-Za-z0-9_-]*\).*/\1/p')"
[ -n "$GSTATE" ] || fail "the timeout hint carries a gs_ state: $ERR"
run agent calendar create "$GC" --title "Late add" --start 2026-10-24T16:00:00Z --end 2026-10-24T17:00:00Z --json
want_rc 0 "a Google change to wait for"
GEV_LATE="$(printf %s "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
run agent calendar wait "$GC" --since-state "$GSTATE" --timeout 30 --json
want_rc 0 "Google wait sees the change"
jok "$OUT" '"'"$GEV_LATE"'" in j["updated"] and j["state"].startswith("gs_") and j["source"] == "google"' "the new event is reported"
run agent calendar wait "$GC" --since-state gd_not-google --timeout 5
want_rc 2 "a foreign state is usage"
LOG="$(fake_log)"
jok "$LOG" 'all("timeMin" not in r["query"] for r in j["requests"] if "syncToken" in r["query"])' "a sync round never carries a window"
ok "wait: full listing → sync token → change reported"

echo "== MCS4: owner token on the Google calendar =="
run owner-ws calendar events "$GC" --from 2026-10-19 --to 2026-10-26 --json
want_rc 0 "owner token reads the Google calendar"
jok "$OUT" '"Late add" in [e["title"] for e in j["events"]]' "owner sees the late add"
run owner-ws calendar delete "$GEV_LATE"
want_rc 0 "owner deletes the late add"
ok "owner: same verbs, same answers"

echo "== MCS4: calendar-only Google links =="
CO="organiser@gmail.example"
glink "$CO" '"calendar"'
jok "$LSTART" '"calendar.events" in j["authorizationUrl"] and "mail.google.com" not in j["authorizationUrl"] and j["features"] == "calendar"' "calendar-only consent skips the restricted Gmail scope"
jok "$LDONE" 'j["state"] == "connected" and "k2 calendar" in j["hint"]' "the calendar-only link connects"
[ "$(sql "SELECT features FROM mail_external_inboxes WHERE email_address='$CO'")" = "calendar" ] || fail "features calendar"
run agent mail capabilities "$CO" --json
want_rc 0 "agent capabilities (calendar-only)"
jok "$OUT" 'j["capabilities"]["calendar"]["read"] and j["capabilities"]["mail"]["read"] is False and any(m["cap"] == "mail.read" and m["reason"] == "scope_not_granted" and m["fix"] == "k2 mail link upgrade organiser@gmail.example --features mail (owner)" for m in j["capabilities"]["missing"])' "calendar on, mail names the upgrade"
run agent calendar events "$CO" --from 2026-10-19 --to 2026-10-26 --json
want_rc 0 "agent reads the calendar-only link's calendar"
run agent mail messages "$CO"
want_rc 3 "mail verbs on a calendar-only link exit 3"
jok "$ERR" 'j["error"]["code"] == "capability_missing" and j["error"]["cap"] == "mail.read" and j["error"]["reason"] == "scope_not_granted"' "capability_missing mail.read"
run agent mail draft --to bob@dest.example --subject "x" --body "y" --from "$CO"
want_rc 3 "no mail drafts on a calendar-only link"
jok "$ERR" 'j["error"]["code"] == "capability_missing" and j["error"]["cap"] == "mail.draft"' "capability_missing mail.draft"
glink "unticked@gmail.example" '"calendar"' "openid https://www.googleapis.com/auth/userinfo.email"
jok "$LDONE" 'j["state"] == "error" and j["code"] == "scope_not_granted"' "a calendar-only consent without the calendar scope fails"
[ "$(sql "SELECT COUNT(*) FROM mail_external_inboxes WHERE email_address='unticked@gmail.example'")" = "0" ] || fail "nothing stored for the unticked consent"
ms_cal="$(curl -s -X POST "$BASE/cli/mail/link/oauth/start?token=$DTOKEN" -H 'Content-Type: application/json' \
    --data-raw "{\"address\":\"x@contoso.example\",\"provider\":\"microsoft\",\"workspace\":\"$REPO\",\"features\":\"calendar\"}")"
jok "$ms_cal" 'j["error"]["code"] == "usage" and "calendar-only links are for Google" in j["error"]["hint"]' "a calendar-only Microsoft link is refused"
ok "calendar-only: calendar works, mail exits 3 with the fix, unticked stores nothing, Microsoft refused"

echo "== MCS4: a mail-only Google link, then upgrade in place =="
MO="mailonly@gmail.example"
glink "$MO" '"mail"'
jok "$LDONE" 'j["state"] == "connected"' "mail-only Google link"
MOROW="$(sql "SELECT id FROM mail_external_inboxes WHERE email_address='$MO'")"
run agent calendar events "$MO"
want_rc 3 "mail-only Google link exits 3 on calendars"
jok "$ERR" 'j["error"]["code"] == "capability_missing" and j["error"]["reason"] == "scope_not_granted" and j["error"]["fix"] == "k2 mail link upgrade mailonly@gmail.example --features calendar (owner)"' "scope_not_granted with the upgrade fix"
up="$(curl -s -X POST "$BASE/cli/mail/link/oauth/start?token=$DTOKEN" -H 'Content-Type: application/json' \
    --data-raw "{\"inbox\":\"$MO\",\"features\":\"calendar\",\"clientCapture\":true,\"redirectUri\":\"http://127.0.0.1:9/cb\"}")"
jok "$up" '"include_granted_scopes=true" in j["authorizationUrl"] and "calendar.events" in j["authorizationUrl"] and "mail.google.com" in j["authorizationUrl"]' "incremental consent asks the union with include_granted_scopes"
scope="$(printf %s "$up" | python3 -c 'import json,sys,urllib.parse as u; d=json.load(sys.stdin); print(u.parse_qs(u.urlparse(d["authorizationUrl"]).query)["scope"][0])')"
fake_cfg "$(python3 -c 'import json,sys; print(json.dumps({"googleAccount": sys.argv[1], "googleScope": sys.argv[2]}))' "$MO" "$scope")"
ulid="$(printf %s "$up" | python3 -c 'import json,sys; print(json.load(sys.stdin)["linkId"])')"
ust="$(printf %s "$up" | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"])')"
curl -s -X POST "$BASE/cli/mail/link/oauth/complete?token=$DTOKEN" -H 'Content-Type: application/json' \
    --data-raw "{\"linkId\":\"$ulid\",\"code\":\"fake-code\",\"state\":\"$ust\"}" >/dev/null
jok "$(curl -s "$BASE/cli/mail/link/oauth/status?token=$DTOKEN&linkId=$ulid")" 'j["state"] == "connected"' "upgrade connects"
[ "$(sql "SELECT id||'|'||features FROM mail_external_inboxes WHERE email_address='$MO'")" = "$MOROW|mail,calendar" ] || fail "same row, mail,calendar"
run agent calendar events "$MO" --from 2026-10-19 --to 2026-10-26 --json
want_rc 0 "the upgraded link's calendar works"
ok "mail-only → upgrade --features calendar in place → k2 calendar works"

echo "== MCS4: Google unlinks =="
for a in "$GC" "$CO" "$MO"; do
    run owner mail link remove "$a"
    want_rc 0 "unlink $a"
done
fake_cfg '{"googleAccount": "rosson@gmail.example", "googleScope": ""}'
jok "$(fake_log)" 'j["sent"] == []' "nothing was sent or added for anyone during MCS4"
ok "Google links removed; nobody was emailed"

echo "== MCS2: Microsoft drafts (compose + reply, attachments, never sent) =="
printf 'quarterly numbers\n' >"$REPO/q.txt"
run agent mail draft --to bob@dest.example --subject "Proposal" --body "draft text" --from "$MS" --attach q.txt
want_rc 0 "agent compose draft on Graph"
run agent mail messages "$MS" --json
want_rc 0 "agent reads the Graph inbox"
MSG_ID="$(printf %s "$OUT" | python3 -c 'import json,sys; v=json.load(sys.stdin); print([m["id"] for m in v["messages"] if m["subject"]=="Hello from Alice"][0])')"
[ -n "$MSG_ID" ] || fail "seeded message id"
run agent mail draft "$MSG_ID" --body "reply draft" --attach q.txt
want_rc 0 "agent reply draft on Graph"
LOG="$(fake_log)"
jok "$LOG" 'any(d.get("subject") == "Proposal" and d["toRecipients"][0]["emailAddress"]["address"] == "bob@dest.example" and [a["name"] for a in d["attachments"]] == ["q.txt"] for d in j["drafts"])' "compose draft with its attachment"
jok "$LOG" 'any(d.get("replyTo") == "msg-1" and d["body"]["content"] == "reply draft" and len(d["attachments"]) == 1 for d in j["drafts"])' "reply draft with its attachment"
jok "$LOG" 'j["sent"] == []' "a draft never sends"
ok "compose + reply drafts with attachments, nothing sent"

echo "== MCS2: approval mode queues a linked send; approve hands it to Graph =="
run agent mail send carol@dest.example --subject "Q3" --body "numbers attached" --from "$MS" --attach q.txt
want_rc 3 "draft level can't send"
run owner mail access level "$MS" "$REPO" --level send
want_rc 0 "owner raises the level to send"
run owner mail sending "$REPO" approval
want_rc 0 "owner sets sending to approval"
run agent mail capabilities "$MS" --json
jok "$OUT" 'j["capabilities"]["mail"]["send"] and j["capabilities"]["mail"]["compose"]' "send + compose on at send level"
run agent mail send carol@dest.example --subject "Q3" --body "numbers attached" --from "$MS" --attach q.txt --json
want_rc 0 "agent send under approval"
jok "$OUT" 'j["status"] == "pending_approval" and j["queued"]' "queued for approval"
OUT_ID="$(printf %s "$OUT" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
jok "$(fake_log)" 'j["sent"] == []' "nothing left before approval"
run agent mail outbox "$OUT_ID" --json
jok "$OUT" 'j["outbound"]["status"] == "pending_approval" and j["outbound"]["attachments"]["names"] == ["q.txt"]' "outbox shows the held send"
run owner hostmail approvals --json
jok "$OUT" 'any(p["id"] == "'"$OUT_ID"'" for p in j["pending"])' "the owner's queue lists it"
run agent hostmail approvals approve "$OUT_ID"
want_rc 3 "an agent can't approve"
run owner hostmail approvals approve "$OUT_ID" --json
want_rc 0 "owner approves"
jok "$OUT" 'j["status"] == "submitted"' "approved → submitted"
LOG="$(fake_log)"
jok "$LOG" 'len(j["sent"]) == 1 and j["sent"][0]["via"] == "sendMail" and j["sent"][0]["saveToSentItems"] is True and j["sent"][0]["message"]["toRecipients"][0]["emailAddress"]["address"] == "carol@dest.example" and [a["name"] for a in j["sent"][0]["message"]["attachments"]] == ["q.txt"]' "Graph sendMail carried the message + attachment"
run agent mail outbox "$OUT_ID" --json
jok "$OUT" 'j["outbound"]["status"] == "submitted"' "outbox: submitted"
ok "approval → approve → Graph sendMail (attachment, saved to Sent)"

echo "== MCS2: gate on — reply over Graph (createReply → send) =="
run owner mail sending "$REPO" on
want_rc 0 "owner sets sending on"
run agent mail reply "$MSG_ID" --body "Friday works" --json
want_rc 0 "agent reply on Graph"
jok "$OUT" 'j["status"] == "submitted" and j["to"] == "alice@example.com" and j["via"] == "graph"' "reply submitted to the locked sender"
LOG="$(fake_log)"
jok "$LOG" 'any(s["via"] == "send" and s["message"]["body"]["content"] == "Friday works" and s["message"]["toRecipients"][0]["emailAddress"]["address"] == "alice@example.com" for s in j["sent"])' "createReply → send"
ok "reply went out via createReply → send"

echo "== MCS2: a >3 MB attachment is refused before anything is audited =="
python3 -c 'import sys; open(sys.argv[1], "wb").write(b"\0" * (3 * 1024 * 1024 + 1))' "$REPO/big.bin"
before="$(sql "SELECT COUNT(*) FROM mail_outbound")"
run agent mail send carol@dest.example --subject "big" --body "x" --from "$MS" --attach big.bin
want_rc 1 "oversize attachment"
has "$ERR" "attachment_too_large" "code attachment_too_large"
[ "$(sql "SELECT COUNT(*) FROM mail_outbound")" = "$before" ] || fail "no audit row for a refused send"
jok "$(fake_log)" 'len(j["sent"]) == 2' "nothing more was sent"
ok "attachment_too_large, no row, nothing sent"

echo "== MCS2: Microsoft unlink =="
run owner mail link remove "$MS"
want_rc 0 "unlink Microsoft"
has "$OUT" "myapps.microsoft.com" "unlink says where to remove consent"
[ "$(sql "SELECT COUNT(*) FROM mail_external_inboxes WHERE email_address='$MS'")" = "0" ] || fail "row gone"
ok "Microsoft unlink explains consent removal"

echo
echo "PASS: $pass checks"
