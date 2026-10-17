#!/usr/bin/env bash
# Mail + calendar surfaces over linked providers (prd-app-mail-calendar-
# surfaces-v1 §8.3), against a REAL headless k2-daemon under a temp HOME and
# the provider fakes (tests/fakes/oauth-providers) — never Google, never
# Microsoft, never the real ~/.k2, the production daemon or the keychain.
#
# Two roles: the owner token, and an agent holding only its session passport
# on the cell socket (minted by a pinned chat whose harness is a shim).
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
#
# Build first (the daemon MUST carry the test-only provider override):
#   cargo build -p k2-provider-fakes
#   cargo build -p k2-daemon --bin k2-daemon --features provider-fakes

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
K2_CLI="$PROJECT_ROOT/cli/k2"
TARGET="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
DAEMON="$TARGET/debug/k2-daemon"
FAKES="$TARGET/debug/k2-provider-fakes"
[ -x "$DAEMON" ] || { echo "FAIL: $DAEMON not built (cargo build -p k2-daemon --features provider-fakes)" >&2; exit 1; }
[ -x "$FAKES" ] || { echo "FAIL: $FAKES not built (cargo build -p k2-provider-fakes)" >&2; exit 1; }
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
for _ in $(seq 1 100); do [ -s "$SB/claude.env" ] && break; sleep 0.05; done
PASSPORT="$(sed -n 's/^K2_HOOK_TOKEN=//p' "$SB/claude.env" | head -1)"
SOCK="$(sed -n 's/^K2_HOOK_SOCK=//p' "$SB/claude.env" | head -1)"
[ -n "$PASSPORT" ] || fail "the pinned chat got no passport"
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
jok "$OUT" 'all(m["cap"] != "calendar.read" or m["reason"] == "not_built" for m in j["capabilities"]["missing"])' "calendar is consented now (not built yet)"
ok "upgrade: same row, mail,calendar consented"

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
