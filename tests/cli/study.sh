#!/usr/bin/env bash
# k2 study — daemon-optional bounded pages (Fair Source, people, errors).
# No daemon: skip-conn-gate matches --schema. Never touch real ~/.k2.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
K2="$K2_CLI"

[ -x "$K2_CLI" ] || { echo "FAIL: $K2_CLI not found/executable" >&2; exit 1; }

pass=0
fail=0
assert_contains() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (missing $(printf %q "$needle"))" >&2
        fail=$((fail + 1))
    fi
}
assert_absent() {
    local label="$1" hay="$2" needle="$3"
    if printf '%s' "$hay" | grep -Fq -- "$needle"; then
        echo "  FAIL: $label (unexpected $(printf %q "$needle"))" >&2
        fail=$((fail + 1))
    else
        echo "  PASS: $label"
        pass=$((pass + 1))
    fi
}
assert_eq() {
    local label="$1" got="$2" want="$3"
    if [ "$got" = "$want" ]; then
        echo "  PASS: $label"
        pass=$((pass + 1))
    else
        echo "  FAIL: $label (got=$(printf %q "$got") want=$(printf %q "$want"))" >&2
        fail=$((fail + 1))
    fi
}

# Fake missing heartbeat: empty HOME, no PORT/TOKEN.
SANDBOX="$(mktemp -d -t k2-study-XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT
export HOME="$SANDBOX"
unset K2_PORT K2_HOOK_TOKEN K2SO_PORT K2SO_HOOK_TOKEN K2_HOST || true

TOPICS="what source map identity send human people auth errors context db mail connect-boundary apps app-heartbeats app-tickets skins feedback-loop ticket-brief zen sidecars llm-tokens llm-accounts publish-framing"

echo "== k2 study source (no daemon) =="
set +e
source_out="$("$K2" study source 2>&1)"
source_rc=$?
set -e
assert_eq "study source exit 0" "$source_rc" "0"
assert_contains "FSL" "$source_out" "FSL"
assert_contains "Fair Source" "$source_out" "Fair Source"
assert_contains "not MIT" "$source_out" "not MIT"

echo "== k2 study app-heartbeats (AH23 / T13: every wait reason) =="
hb_out="$("$K2" study app-heartbeats)"
WAIT_FIXTURE="$PROJECT_ROOT/crates/k2-core/src/heartbeats/fixtures/wait-reasons.json"
[ -f "$WAIT_FIXTURE" ] || { echo "FAIL: $WAIT_FIXTURE missing" >&2; exit 1; }
reasons="$(python3 -c 'import json,sys; print("\n".join(json.load(open(sys.argv[1]))))' "$WAIT_FIXTURE")"
[ -n "$reasons" ] || { echo "FAIL: no wait reasons in fixture" >&2; exit 1; }
reason_count=0
while IFS= read -r reason; do
    reason_count=$((reason_count + 1))
    if printf '%s\n' "$hb_out" | grep -Eq "^  ${reason}[[:space:]]"; then
        echo "  PASS: wait reason $reason has a row"
        pass=$((pass + 1))
    else
        echo "  FAIL: wait reason $reason missing from k2 study app-heartbeats" >&2
        fail=$((fail + 1))
    fi
done <<< "$reasons"
assert_eq "fixture lists 14 reasons" "$reason_count" "14"
assert_contains "list route" "$hb_out" "GET  /cli/heartbeat/list?workspace="
assert_contains "show route" "$hb_out" "GET  /cli/heartbeat/show?workspace=&name="
assert_contains "fire route" "$hb_out" "POST /cli/heartbeat/fire"
assert_contains "archive is remove" "$hb_out" "POST /cli/heartbeat/archive"
assert_contains "socket frame" "$hb_out" '{"kind":"heartbeat_changed","workspace":"<handle>"}'
assert_contains "files:read gates body" "$hb_out" "instructionsHidden"
assert_contains "count down rule" "$hb_out" "Count down from nextFireAt"
assert_contains "refetch +120s" "$hb_out" "nextFireAt + 120 s"
assert_contains "fire resets interval" "$hb_out" "Fire now resets an interval heartbeat"
assert_contains "failed fire counts (AH34)" "$hb_out" "5-failure auto-disable"
assert_contains "rate limit" "$hb_out" "12 per"
assert_contains "retry-after" "$hb_out" "Retry-After"
assert_contains "core does catch-up" "$hb_out" "under 12 h old"
assert_contains "actor values" "$hb_out" "app-token:<name>"
assert_absent "never scheduler-status for apps" "$hb_out" "GET  /cli/heartbeat/scheduler-status"
hb_json="$("$K2" study app-heartbeats --json)"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["id"]=="app-heartbeats" and "no_ticks" in d["body"], d["id"]' "$hb_json"
echo "  PASS: app-heartbeats --json id+body"
pass=$((pass + 1))

echo "== k2 study app-tickets (prd-app-tickets-websocket-v1) =="
tk_out="$("$K2" study app-tickets)"
assert_contains "tickets socket path" "$tk_out" "WS   /cli/activity/events?workspace=<handle>"
assert_contains "ticket_changed frame" "$tk_out" '{"kind":"ticket_changed","workspace":"<handle>","id":"<id>",'
for change in created commented answered status_changed assigned resolved dismissed brief_attached; do
    if printf '%s\n' "$tk_out" | grep -Eq "^  ${change}[[:space:]]"; then
        echo "  PASS: change $change has a row"
        pass=$((pass + 1))
    else
        echo "  FAIL: change $change missing from k2 study app-tickets" >&2
        fail=$((fail + 1))
    fi
done
assert_contains "via option_pick" "$tk_out" "option_pick"
assert_contains "via free_text" "$tk_out" "free_text"
assert_contains "option pick answers" "$tk_out" "optionPick:true"
assert_contains "free text discusses" "$tk_out" "needs_discussion"
assert_contains "assign route" "$tk_out" "POST /cli/feedback/assign"
assert_contains "assign warning" "$tk_out" "assignee_unknown"
assert_contains "brief on demand" "$tk_out" "show?id=<id>&brief=1"
assert_contains "brief never on the socket" "$tk_out" "The brief is never on the socket."
assert_contains "no backfill" "$tk_out" "backfill or replay"
assert_contains "refetch on reopen" "$tk_out" "On every socket (re)open, refetch once."
assert_contains "apps cannot set answered" "$tk_out" "answered is refused for apps"
assert_contains "no 404 oracle" "$tk_out" "no 404 oracle"
assert_absent "never list-all for apps" "$tk_out" "GET  /cli/feedback/list-all"
tk_json="$("$K2" study app-tickets --json)"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["id"]=="app-tickets" and "ticket_changed" in d["body"], d["id"]' "$tk_json"
echo "  PASS: app-tickets --json id+body"
pass=$((pass + 1))

echo "== k2 study skins thread/overlay contract =="
skins_out="$("$K2" study skins)"
assert_contains "thread addr param" "$skins_out" "GET /cli/thread?addr="
assert_contains "not agent query" "$skins_out" "not ?agent="
assert_contains "conversation_id" "$skins_out" "conversation_id"
assert_contains "item.doc.text" "$skins_out" "item.doc.text"
assert_contains "overlay conversation query" "$skins_out" "WS /cli/overlay/events?conversation="
assert_contains "post addr text" "$skins_out" '{"addr":"<handle>","text":"…"}'
assert_absent "gateway no files this cut" "$skins_out" "does NOT proxy /cli/fs/*"
assert_contains "files read-dir" "$skins_out" "/cli/fs/read-dir?workspace="
assert_contains "files read-file" "$skins_out" "/cli/fs/read-file?workspace="
assert_contains "files read-binary" "$skins_out" "/cli/fs/read-binary?workspace="
assert_contains "files read-range" "$skins_out" "/cli/fs/read-range?workspace="
assert_contains "files write-file" "$skins_out" "/cli/fs/write-file"
assert_contains "files upload-binary" "$skins_out" "/cli/fs/upload-binary"
assert_contains "files create" "$skins_out" "/cli/fs/create"
assert_contains "files copy" "$skins_out" "/cli/fs/copy"
assert_contains "files move" "$skins_out" "/cli/fs/move"
assert_contains "ensure-pinned-chat" "$skins_out" "/cli/workspace/ensure-pinned-chat"
assert_contains "files events workspace" "$skins_out" "/cli/fs/events?workspace="
assert_contains "never fs glob" "$skins_out" "Never /cli/fs/*"
assert_contains "never owner BFF" "$skins_out" "Never owner BFF"
assert_contains "has_cap_in_room doors" "$skins_out" "has_cap_in_room"
assert_contains "static dir is public" "$skins_out" "GET / and /assets/* are PUBLIC"
assert_contains "platform mint --name" "$skins_out" "k2 app token create --name"
assert_contains "leftover mint --name" "$skins_out" "k2 skin-token create --name"
assert_contains "two credentials" "$skins_out" "TWO CREDENTIALS"
assert_contains "byo bff heading" "$skins_out" "BYO BFF (not --skin)"
assert_contains "byo same session" "$skins_out" "same session k2skn_"
assert_contains "byo bearer" "$skins_out" "Authorization: Bearer"
assert_contains "byo not platform for dentist" "$skins_out" "Never a platform --name token for \"this"
assert_contains "login json principalId" "$skins_out" "principalId"
assert_contains "roles for guests" "$skins_out" "ROLES (named bundles for guests"
assert_contains "platform token is not a role" "$skins_out" "it is not a role"
assert_contains "empty rooms dark" "$skins_out" "Empty rooms on a role = Thread dark"
assert_contains "find the room" "$skins_out" "FIND THE ROOM, THEN THE FUNCTIONS"
assert_contains "files on docs not sales" "$skins_out" "Files on Documents does not grant files on Sales"
assert_contains "role room example" "$skins_out" "k2 app role room dentist sales"
assert_absent "no leftover cartesian create" "$skins_out" "k2 skin role create dentist --caps"
assert_absent "files later cut" "$skins_out" "later gateway cut"
assert_absent "no mint-for-user" "$skins_out" "skin-token create <username>"
assert_contains "tickets:read scope" "$skins_out" "tickets:read"
assert_contains "tickets:post scope" "$skins_out" "tickets:post"
assert_contains "tickets list path" "$skins_out" "/cli/feedback/list?project="
assert_contains "tickets show path" "$skins_out" "/cli/feedback/show?id="
assert_contains "tickets create path" "$skins_out" "/cli/feedback/create"
assert_contains "tickets comment path" "$skins_out" "/cli/feedback/comment"
assert_contains "tickets answer path" "$skins_out" "/cli/feedback/answer"
assert_contains "tickets resolve path" "$skins_out" "/cli/feedback/resolve"
assert_contains "tickets assign path" "$skins_out" "/cli/feedback/assign"
assert_contains "tickets live pointer" "$skins_out" "k2 study app-tickets"
assert_contains "tickets on docs not sales" "$skins_out" "tickets on Documents does not grant tickets on Sales"
assert_contains "tickets project handle" "$skins_out" "project= is handle or uuid only"
assert_contains "wiki:read scope" "$skins_out" "wiki:read"
assert_contains "wiki index path" "$skins_out" "/cli/wiki/index?project="
assert_contains "wiki note path" "$skins_out" "/cli/wiki/note?project="
assert_contains "wiki on docs not sales" "$skins_out" "wiki on Documents does not grant wiki on Sales"
assert_contains "store:read scope" "$skins_out" "store:read"
assert_contains "store:write scope" "$skins_out" "store:write"
assert_contains "store list path" "$skins_out" "/cli/store/list?workspace="
assert_contains "store get path" "$skins_out" "/cli/store/get?workspace="
assert_contains "store query path" "$skins_out" "/cli/store/query?workspace="
assert_contains "dump tables path" "$skins_out" "/cli/db/tables?workspace="
assert_contains "dump rows path" "$skins_out" "/cli/db/rows?workspace="
assert_contains "dump update path" "$skins_out" "/cli/db/rows/update"
assert_contains "dump delete path" "$skins_out" "/cli/db/rows/delete"
assert_contains "store write not put" "$skins_out" "not POST /cli/store/put"
assert_contains "store guc" "$skins_out" "set_config('k2.skin_principal'"
assert_contains "skin uid helper" "$skins_out" "k2.skin_uid()"
assert_contains "store on docs not sales" "$skins_out" "store on Documents does not grant store on Sales"
assert_contains "never dsn in spa" "$skins_out" "GET /cli/db/dsn teaching 403"
assert_contains "never db list" "$skins_out" "Never /cli/db/list"
assert_contains "never db dump" "$skins_out" "Never /cli/db/dump"
assert_contains "never db migrate" "$skins_out" "Never /cli/db/migrate"
assert_contains "never db foo" "$skins_out" "Never /cli/db/foo"
assert_absent "no glob db star" "$skins_out" "Never /cli/db/*"
assert_absent "no dsn body" "$skins_out" "postgres://"
assert_absent "no chatter scope" "$skins_out" "chatter"
assert_contains "custom login.html" "$skins_out" "login.html"
assert_contains "guest card answer" "$skins_out" "POST /cli/thread/answer"
assert_contains "agents ask from PTY" "$skins_out" "k2 thread ask"
assert_contains "agents secret from PTY" "$skins_out" "k2 thread secret"
assert_contains "owner vs agent heading" "$skins_out" "OWNER VS AGENT"
assert_contains "list always" "$skins_out" "workspace agent always"
assert_contains "agent tab toggle" "$skins_out" "Allow this agent to manage Apps"
assert_contains "leftover front-door owner" "$skins_out" "front-door"
assert_contains "leftover hydra owner" "$skins_out" "hydra"
assert_contains "do not sudo" "$skins_out" "Do not sudo"
assert_contains "password forgot path" "$skins_out" "POST /cli/skin/password/forgot"
assert_contains "password reset path" "$skins_out" "POST /cli/skin/password/reset"
assert_contains "password change path" "$skins_out" "POST /cli/skin/password/change"
assert_contains "mint platform or owner" "$skins_out" "Mint = platform --name or owner"
assert_contains "email optional guests" "$skins_out" "Email is optional"
assert_contains "mint needs password and email" "$skins_out" "password AND an email"
assert_contains "bff uses forgot json email" "$skins_out" "forgot JSON email"
assert_contains "forgot hit expiresAt" "$skins_out" "expiresAt"
assert_contains "consume public" "$skins_out" "PUBLIC consume"
assert_contains "change session only" "$skins_out" "session k2skn_ only"
assert_contains "delivery is the operator" "$skins_out" "Delivery is the operator BYO BFF"
assert_contains "never k2 mail guests" "$skins_out" "Never k2 mail"
assert_contains "never owner platform in browser" "$skins_out" "Never put an owner or platform token in the browser"
assert_contains "bff must not return token" "$skins_out" "BFF must not return forgot JSON token"
assert_contains "official consume post" "$skins_out" "Official consume POST"
assert_contains "forgot 404 on skin" "$skins_out" "Official --skin: this path 404s"
assert_contains "oidc this password file" "$skins_out" "OIDC issuer still this password file"
assert_contains "rp google at idp" "$skins_out" "RP/Google reset at the IdP"
assert_absent "no teammate names in skins" "$skins_out" "Dannon"
assert_absent "no resend product in skins" "$skins_out" "Resend"

echo "== k2 study apps is canonical; skins is leftover alias =="
"$K2" study apps >"$SANDBOX/apps.txt"
"$K2" study skins >"$SANDBOX/skins.txt"
apps_out="$(cat "$SANDBOX/apps.txt")"
skins_alias_out="$(cat "$SANDBOX/skins.txt")"
assert_contains "apps page title" "$apps_out" "k2 study apps — Apps"
assert_absent "apps has no leftover alias line" "$apps_out" "k2 study skins is now k2 study apps"
assert_contains "skins leftover alias line" "$skins_alias_out" "k2 study skins is now k2 study apps — this page still works."
if python3 - "$SANDBOX/apps.txt" "$SANDBOX/skins.txt" <<'PY'
import pathlib, sys
apps = pathlib.Path(sys.argv[1]).read_text()
skins = pathlib.Path(sys.argv[2]).read_text()
lines = skins.splitlines()
while lines and (lines[0].startswith("k2 study skins is now") or lines[0] == ""):
    lines.pop(0)
skins_body = "\n".join(lines)
if skins_body.endswith("\n") is False and apps.endswith("\n"):
    skins_body += "\n"
if apps.rstrip("\n") != skins_body.rstrip("\n"):
    sys.exit(1)
PY
then
    echo "  PASS: skins body matches apps after alias line"
    pass=$((pass + 1))
else
    echo "  FAIL: skins body matches apps after alias line" >&2
    fail=$((fail + 1))
fi
assert_contains "apps keeps k2skn_" "$apps_out" "k2skn_"
assert_contains "apps keeps /cli/skin/login" "$apps_out" "/cli/skin/login"

echo "== k2 study apps: agent working (prd-daemon-activity-and-thread-working-v1 AP4) =="
assert_contains "activity section" "$apps_out" "AGENT WORKING (activity:read)"
assert_contains "guest snapshot path" "$apps_out" "GET      /cli/activity/snapshot?workspace=<handle>"
assert_contains "activity socket path" "$apps_out" "WS GET   /cli/activity/events?workspace=<handle>"
assert_contains "permission word" "$apps_out" "permission  the agent waits for a person to approve a step"
assert_contains "unknown word" "$apps_out" "unknown     no update from the agent for 30 minutes"
assert_contains "since documented" "$apps_out" '"since":1781812201000|null'
assert_contains "serverNow documented" "$apps_out" '"serverNow":1781812273000}'
assert_contains "new words flagged" "$apps_out" "permission and unknown are NEW words"
assert_contains "resync re-pull" "$apps_out" '{"kind":"resync"}'
assert_contains "no tool detail for apps" "$apps_out" "No tool names, commands, files, thinking text"
assert_contains "counts documented" "$apps_out" '"counts":{"subagents":2,"tools":14,"commands":5}'
assert_contains "counts reset per turn" "$apps_out" "tools and commands go back to 0 when a new turn starts"
assert_contains "counts throttle" "$apps_out" "A change to"
assert_contains "counts throttle rate" "$apps_out" "counts alone goes out at most once a second per session"
assert_contains "never the strip catch-up" "$apps_out" "Never /cli/thread/activity"
assert_contains "app-tickets points at the section" "$tk_out" "AGENT WORKING"

echo "== k2 study ticket-brief (prd-ticket-html-brief-v1 T13) =="
tb_out="$("$K2" study ticket-brief)"
assert_contains "ticket-brief title" "$tb_out" "k2 study ticket-brief"
assert_contains "template verb" "$tb_out" "k2 tickets template > brief.html"
assert_contains "ask --html" "$tb_out" "--html brief.html"
assert_contains "stdin form" "$tb_out" "--html - < brief.html"
assert_contains "k2-need section" "$tb_out" 'class="k2-need"'
assert_contains "k2-options list" "$tb_out" 'class="k2-options"'
assert_contains "1 MiB cap" "$tb_out" "1 MiB"
assert_contains "fyi exempt" "$tb_out" "fyi"
assert_contains "warn this release" "$tb_out" "brief_missing"
assert_contains "require next release" "$tb_out" "brief_required"
assert_contains "script stripped" "$tb_out" "script"
assert_contains "links listed" "$tb_out" "system browser"
assert_contains "show --html" "$tb_out" "k2 tickets show <id> --html"
assert_contains "skins brief read" "$skins_out" "/cli/feedback/show?id=<id>&brief=1"
assert_contains "skins brief create optional" "$skins_out" '"briefHtml"'
assert_contains "skins brief sandbox" "$skins_out" 'sandbox=""'
assert_contains "skins brief csp" "$skins_out" "default-src 'none'; img-src data:"
fl_out="$("$K2" study feedback-loop)"
assert_contains "feedback-loop teaches the brief" "$fl_out" "k2 study ticket-brief"
tb_json="$("$K2" study ticket-brief --json)"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["id"]=="ticket-brief" and "k2-need" in d["body"], d["id"]' "$tb_json"
echo "  PASS: ticket-brief --json id+body"
pass=$((pass + 1))

echo "== k2 study sidecars =="
sc_out="$("$K2" study sidecars)"
assert_contains "sidecar verbs" "$sc_out" "k2 sidecar new <name> --harness <preset>"
assert_contains "stop is sleep" "$sc_out" "stop is sleep"
assert_contains "brief file" "$sc_out" ".k2/sidecars/<name>/BRIEF.md"
assert_contains "switch named" "$sc_out" "Allow hiring and managing"
assert_contains "guardrail sentence" "$sc_out" "guardrails, not locks"
assert_contains "handback" "$sc_out" "k2 msg <primary>"

echo "== k2 study llm-tokens =="
la_out="$("$K2" study llm-tokens)"
assert_contains "title" "$la_out" "k2 study llm-tokens — LLM tokens: subscriptions and API tokens"
assert_contains "one server default" "$la_out" "ONE server default token per server"
assert_contains "subscription defined" "$la_out" "SUBSCRIPTION"
assert_contains "api token defined" "$la_out" "TOKEN (a provider key, billed per token)"
assert_contains "token folder path" "$la_out" "~/.k2/llm-accounts/<tool>/<id>/"
assert_contains "every chat on Server default" "$la_out" "Changing the server default affects every chat that uses Server"
assert_contains "never the server default one" "$la_out" "refreshes the server default one."
assert_contains "air-gap" "$la_out" "Under air-gap"
assert_contains "read-only terminals" "$la_out" "Agents and K2 terminals are read-only"
assert_contains "not a security boundary" "$la_out" "security boundary."
assert_contains "bundles never carry it" "$la_out" "never carry the tokens"
assert_contains "no auto rotation" "$la_out" "never rotates tokens"
assert_contains "workspace/chat section" "$la_out" "A TOKEN FOR ONE WORKSPACE OR CHAT"
assert_contains "pick action wording" "$la_out" "Use this token for this workspace / this chat"
assert_contains "precedence" "$la_out" "its own token > its workspace's token > Server"
assert_contains "a chat's own token sticks" "$la_out" "A chat's own token sticks to that chat: it applies every time"
assert_contains "resume keeps its token" "$la_out" "Without its own token, a resumed chat keeps the token it started on;"
assert_contains "api tokens billed per token" "$la_out" "billed per token"
assert_contains "cli noun" "$la_out" "k2 llm tokens add <tool> <label> [--device]"
assert_absent_word() {
    if printf '%s\n' "$2" | grep -iqw -- "$3"; then
        echo "  FAIL: $1 (matched $3)" >&2
        fail=$((fail + 1))
    else
        echo "  PASS: $1"
        pass=$((pass + 1))
    fi
}
assert_absent_word "no wallet word" "$la_out" "wallet"
assert_absent_word "no pool word" "$la_out" "pool"
la_json="$("$K2" study llm-tokens --json)"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["id"]=="llm-tokens" and "needs_login" in d["body"], d["id"]' "$la_json"
echo "  PASS: llm-tokens --json id+body"
pass=$((pass + 1))

echo "== k2 study llm-accounts (alias) =="
alias_out="$("$K2" study llm-accounts)"
assert_contains "alias names the new page" "$alias_out" "k2 study llm-accounts is now k2 study llm-tokens — this page still works."
assert_eq "alias prints the same page after its note" "$(printf '%s\n' "$alias_out" | tail -n +3)" "$la_out"
alias_json="$("$K2" study llm-accounts --json)"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["id"]=="llm-accounts" and "k2 study llm-tokens" in d["body"], d["id"]' "$alias_json"
echo "  PASS: llm-accounts --json id+body"
pass=$((pass + 1))

echo "== k2 study people =="
people_out="$("$K2" study people)"
assert_contains "connections (agents)" "$people_out" "CONNECTIONS (AGENTS)"
assert_contains "Connect users" "$people_out" "CONNECT USERS"
assert_contains "app guests" "$people_out" "APP GUESTS"
assert_contains "leftover skin user list" "$people_out" "k2 skin user list"
assert_contains "--users humans" "$people_out" "--users"
assert_contains "never k2 msg" "$people_out" "never"
assert_contains "k2 msg names" "$people_out" "k2 msg"

echo "== k2 study errors =="
errors_out="$("$K2" study errors)"
assert_contains "exit 3" "$errors_out" "EXIT 3"
assert_contains "owner_only" "$errors_out" "owner_only"
assert_contains "stuck bootstrap" "$errors_out" "already initialized"
assert_contains "stuck bootstrap disable" "$errors_out" "k2 hostmail disable"

echo "== k2 study send =="
send_out="$("$K2" study send)"
assert_contains "k2 msg" "$send_out" "k2 msg"
assert_contains "k2 thread" "$send_out" "k2 thread"
assert_contains "k2 mail" "$send_out" "k2 mail"

echo "== k2 study mail =="
mail_out="$("$K2" study mail)"
assert_contains "hostmail owner" "$mail_out" "k2 hostmail"
assert_contains "disable host-wide" "$mail_out" "DISABLE IS HOST-WIDE"
assert_contains "systemctl is-active" "$mail_out" "systemctl is-active stalwart"
assert_contains "supervisor writes" "$mail_out" "k2 hostmail enable"
assert_contains "cert owner section" "$mail_out" "WHO OWNS THE MAIL CERTIFICATE"
assert_contains "cert.owner in status" "$mail_out" "cert.owner"
assert_contains "k2 owner defined by attach + planted" "$mail_out" "the mail certificate is one K2's issuer planted"
assert_absent "owner never defined by zone on K2 DNS" "$mail_out" "zone on K2 DNS"
assert_contains "stalwart ACME manual" "$mail_out" "Manual for that domain: do not turn it back on"
assert_contains "renew now" "$mail_out" "k2 hostmail cert renew"
assert_contains "own certs per extra name" "$mail_out" "get their OWN K2 certificate"
assert_contains "apex and www never" "$mail_out" "The apex and www are never on a mail certificate."
assert_contains "doctor k2-cert-renewal" "$mail_out" "k2-cert-renewal"
assert_contains "doctor acme-cert-names manual" "$mail_out" "Manual (K2 owns and renews this"
assert_contains "pre-0.45.0 empty list expected" "$mail_out" "not yet on 0.45.0"
assert_contains "flip-back next to acme-cert-names" "$mail_out" "k2 hostmail cert owner --stalwart-acme"
assert_contains "doctor extra-name-certs" "$mail_out" "extra-name-certs"
assert_contains "doctor tls-cert" "$mail_out" "tls-cert          what :443 serves right now."
assert_contains "planted not loaded remedy" "$mail_out" "sudo systemctl restart stalwart"
assert_contains "never disable/enable for certs" "$mail_out" "Never fix a certificate with \`k2 hostmail disable/enable\`."
acme_line="$(printf '%s\n' "$mail_out" | grep -n 'acme-cert-names ' | head -1 | cut -d: -f1)"
flip_line="$(printf '%s\n' "$mail_out" | grep -n 'k2 hostmail cert owner --stalwart-acme' | head -1 | cut -d: -f1)"
if [ -n "$acme_line" ] && [ -n "$flip_line" ] && [ $((flip_line - acme_line)) -ge 0 ] && [ $((flip_line - acme_line)) -le 8 ]; then
    echo "  PASS: flip-back command sits with the acme-cert-names line"
    pass=$((pass + 1))
else
    echo "  FAIL: flip-back command must sit with the acme-cert-names line ($acme_line/$flip_line)" >&2
    fail=$((fail + 1))
fi

echo "== k2 study nosuch =="
set +e
nosuch_out="$("$K2" study nosuch 2>&1)"
nosuch_rc=$?
set -e
assert_eq "unknown topic exit 2" "$nosuch_rc" "2"
assert_contains "unknown topic usage" "$nosuch_out" "Usage: k2 study"
assert_contains "lists valid ids" "$nosuch_out" "feedback-loop"

echo "== k2 study --json catalog =="
json_catalog="$("$K2" study --json)"
json_list="$("$K2" study list --json)"
python3 -c 'import json,sys; json.loads(sys.argv[1])' "$json_catalog"
python3 -c 'import json,sys; json.loads(sys.argv[1])' "$json_list"
echo "  PASS: catalog JSON parses"
pass=$((pass + 1))
echo "  PASS: list --json JSON parses"
pass=$((pass + 1))
for id in $TOPICS; do
    assert_contains "catalog has $id" "$json_catalog" "\"id\":\"$id\""
    assert_contains "list --json has $id" "$json_list" "\"id\":\"$id\""
done

echo "== k2 study source --json =="
source_json="$("$K2" study source --json)"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d.get("id")=="source" and "FSL" in d.get("body",""), d' "$source_json"
echo "  PASS: source --json id+body"
pass=$((pass + 1))

echo "== no this-box hostnames / raw secrets =="
all="$("$K2" study)"
all="$all$("$K2" study list)"
all="$all$json_catalog"
for id in $TOPICS; do
    all="$all$("$K2" study "$id")"
done
assert_absent "no rosson.k2.dev" "$all" "rosson.k2.dev"
assert_absent "no z3mbp" "$all" "z3mbp"
if printf '%s' "$all" | grep -Eq 'k2skn_[A-Za-z0-9]{8,}'; then
    echo "  FAIL: raw k2skn_ secret material" >&2
    fail=$((fail + 1))
else
    echo "  PASS: no raw k2skn_ secret material"
    pass=$((pass + 1))
fi

echo "== k2 study publish-framing (prd-app-frame-ancestors-v1 FA21) =="
pf_out="$("$K2" study publish-framing)"
assert_contains "title" "$pf_out" "k2 study publish-framing"
assert_contains "app default policy" "$pf_out" "frame-ancestors 'self' tauri://localhost http://tauri.localhost"
assert_contains "allow verb" "$pf_out" "k2 publish frame <name> --allow https://your.site"
assert_contains "none verb" "$pf_out" "k2 publish frame <name> --none"
assert_contains "reset verb" "$pf_out" "k2 publish frame <name> --reset"
assert_contains "owner only" "$pf_out" "owner_only, exit 3"
assert_contains "never k2.dev wildcard" "$pf_out" "Never allow *.k2.dev"
assert_contains "cmd apps untouched" "$pf_out" "cannot add the"
assert_contains "next headers" "$pf_out" "async headers()"
assert_contains "express header" "$pf_out" "res.set('Content-Security-Policy', \"frame-ancestors 'self'\")"
assert_contains "node header" "$pf_out" "res.setHeader('Content-Security-Policy', \"frame-ancestors 'self'\")"
pf_json="$("$K2" study publish-framing --json)"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["id"]=="publish-framing" and "frame-ancestors" in d["body"], d["id"]' "$pf_json"
echo "  PASS: publish-framing --json id+body"
pass=$((pass + 1))

echo "== --help exit 0; bad flag exit 2 =="
set +e
help_out="$("$K2" study --help 2>&1)"
help_rc=$?
bad_out="$("$K2" study --bogus 2>&1)"
bad_rc=$?
unknown_verb_out="$("$K2" nosuch 2>&1)"
unknown_verb_rc=$?
help_gate_out="$("$K2" help 2>&1)"
help_gate_rc=$?
set -e
assert_eq "study --help exit 0" "$help_rc" "0"
assert_contains "study --help usage" "$help_out" "Usage: k2 study"
assert_eq "bad flag exit 2" "$bad_rc" "2"
assert_eq "unknown live verb still needs PORT/TOKEN" "$unknown_verb_rc" "1"
assert_contains "unknown verb connection error" "$unknown_verb_out" "Cannot connect to K2"
assert_eq "k2 help is not daemon-optional" "$help_gate_rc" "1"

echo "== schema mentions study =="
assert_contains "schema usage" "$(grep -F 'k2 study [list' "$K2_CLI" || true)" "k2 study [list|<topic>|--json]"

echo ""
echo "Results: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
