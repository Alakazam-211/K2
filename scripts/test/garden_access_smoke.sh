#!/usr/bin/env bash
# Garden access, headless and phone smoke (prd-garden-access-v1 rev 5, A6).
#
# Builds this checkout's k2-daemon, runs it from `env -i` in a temp HOME
# (every K2_*/K2SO_* dropped; never the real ~/.k2, the keychain, the
# running daemon or the tunnel) and drives it with curl only.
#
# The agents are FAKES: a `claude` shim (so the daemon classes the session
# as an agent) that writes its session passport ($K2_HOOK_TOKEN) to
# a file and sleeps. Two workspaces: alpha (makes a Garden, gets listed)
# and bravo (never listed). The person is the owner token; the phone is an
# Owner login (`/cli/auth/login`).
#
#   scripts/test/garden_access_smoke.sh          (KEEP=1 keeps the temp dir)
#   GA_SMOKE_DUMP=1 …                             also prints every answer it
#                                                 kept, as fixtures for the
#                                                 renderer's client tests
#
# Steps (each check is PASS or FAIL; any FAIL exits 1 at the end):
#   0  switch on; 0.46.3 routes answer as before while off
#   1  alpha makes a Garden (page + widget): `made`, the garden.access card
#   2  the person answers Allow on the card (the Thread answer route)
#   3  alpha edits (garden/put, widget/folder/put): 200, stamped, signed
#   4  bravo is refused (403 garden_access_needed + card) but is still used
#      in the Garden (a widget-origin Thread post to bravo succeeds)
#   5  lock (423 for alpha) and unlock; design updates off
#   6  alpha asks to delete (403 garden_delete_person + garden.delete card);
#      the phone (Owner login, cards=2 view) answers Allow: deleted, listed
#      in Deleted Gardens, restored with its editors
#   7  Sign: a raw disk edit is "not signed" in History; the person signs
#   8  History shows every step (by kind); View this version
#   9  switch off: the new routes are unknown, agent garden/new is 0.46.3's
#
# History lines written by slice A6 itself (locked, unlocked, sync,
# disk_edit) are checked in step 8 like every other kind.
set -uo pipefail
for t in curl jq cargo; do command -v "$t" >/dev/null || { echo "[ga-smoke] FAIL: needs $t" >&2; exit 1; }; done
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
say() { printf '[ga-smoke %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

if [[ "${GA_SMOKE_NO_BUILD:-0}" != 1 ]]; then
  say "building k2-daemon"
  (cd "$REPO" && cargo build -q -p k2-daemon --bin k2-daemon) || { say "FAIL: build"; exit 1; }
fi
BIN="$(cd "$REPO" && cargo metadata -q --format-version 1 --no-deps | jq -r .target_directory)/debug/k2-daemon"
[[ -x "$BIN" ]] || { say "FAIL: no daemon binary at $BIN"; exit 1; }

ROOT="$(mktemp -d /tmp/k2ga-XXXX)"
H="$ROOT/h"
FIX="$ROOT/fixtures"
PATH_CLEAN="/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin"
mkdir -p "$H/.k2" "$H/tmp" "$H/run" "$H/agent-shim" "$H/alpha" "$H/bravo" "$FIX"
chmod 700 "$H/tmp" "$H/run"
PID=""
pass=0
fail=0
FAILED=()
ok() { say "  PASS: $1"; pass=$((pass + 1)); }
bad() { say "  FAIL: $1"; fail=$((fail + 1)); FAILED+=("$1"); }
die() {
  say "FATAL: $*"
  say "daemon.log (last 60 lines):"
  tail -60 "$H/daemon.log" >&2 || true
  exit 1
}
cleanup() {
  [[ -n "$PID" ]] && kill "$PID" 2>/dev/null
  sleep 0.3
  [[ -n "$PID" ]] && kill -9 "$PID" 2>/dev/null
  pkill -9 -f "$H/agent-shim/claude" 2>/dev/null
  if [[ "${KEEP:-0}" == 1 ]]; then say "kept $ROOT"; else rm -rf "$ROOT"; fi
}
trap cleanup EXIT

# ── the daemon ───────────────────────────────────────────────────────

env -i HOME="$H" PATH="$PATH_CLEAN" USER="$(id -un)" LANG=C.UTF-8 SHELL=/bin/sh TMPDIR="$H/tmp" XDG_RUNTIME_DIR="$H/run" \
  K2_TEST_AGENT_SHIM_DIR="$H/agent-shim" K2SO_WATCHDOG_DISABLED=1 K2_HEARTBEAT_NO_SELF_HEAL=1 \
  K2_SUBSCRIPTION_PROBE=deny K2_AIRGAP=1 K2_TUNNEL_LEASE=off K2_HOOK_INSTALL=0 \
  K2_LLM_ACCOUNTS_KEYCHAIN=0 K2_LLM_ACCOUNTS_LOOP=0 K2_USAGE_PRICES_FETCH=0 \
  "$BIN" >>"$H/daemon.log" 2>&1 &
PID=$!
for _ in $(seq 1 300); do
  if [[ -s "$H/.k2/daemon.token" && -s "$H/.k2/daemon.port" ]] &&
    curl -fsS "http://127.0.0.1:$(cat "$H/.k2/daemon.port")/boot-status" 2>/dev/null | jq -e '.phase=="ready"' >/dev/null; then break; fi
  kill -0 "$PID" 2>/dev/null || die "daemon exited"
  sleep 0.2
done
PORT="$(cat "$H/.k2/daemon.port")"
OWNER="$(cat "$H/.k2/daemon.token")"
U="http://127.0.0.1:$PORT"
say "daemon up on :$PORT"

# call TOKEN METHOD PATH [JSON] [QUERY] → sets CODE and BODY (never dies).
call() {
  local tok="$1" method="$2" path="$3" body="${4:-}" query="${5:-}" out
  out="$(curl -sS -X "$method" "$U$path?token=$tok${query:+&$query}" \
    -H 'content-type: application/json' -H 'Origin: tauri://localhost' ${body:+--data "$body"} -w $'\n%{http_code}')" || out=$'curl failed\n000'
  CODE="${out##*$'\n'}"
  BODY="${out%$'\n'*}"
}
# must TOKEN METHOD PATH [JSON] [QUERY]: call and die on non-2xx (setup only).
must() {
  call "$@"
  [[ "$CODE" == 2?? ]] || die "$2 $3 → $CODE: $BODY"
}
# want WHAT CODE [jq-filter]: check the last call's status (and a jq truth).
want() {
  local what="$1" code="$2" filter="${3:-}"
  if [[ "$CODE" != "$code" ]]; then
    bad "$what: HTTP $CODE, want $code: $BODY"
    return 1
  fi
  if [[ -n "$filter" ]] && ! jq -e "$filter" >/dev/null 2>&1 <<<"$BODY"; then
    bad "$what: $filter is not true of: $BODY"
    return 1
  fi
  ok "$what"
}
keep() { printf '%s\n' "$BODY" >"$FIX/$1.json"; }
uri() { jq -rn --arg v "$1" '$v|@uri'; }

# ── people, agents, Zen ──────────────────────────────────────────────

PW="ga-smoke-Pass-1234!"
must "$OWNER" POST /cli/users/add "{\"username\":\"phone\",\"password\":\"$PW\"}"
must "$OWNER" POST /cli/users/set-role '{"username":"phone","role":"owner"}'
must "$OWNER" POST /cli/auth/login "{\"username\":\"phone\",\"password\":\"$PW\"}"
LOGIN="$(jq -r '.token' <<<"$BODY")"
[[ -n "$LOGIN" && "$LOGIN" != null ]] || die "no login token: $BODY"
say "Owner login 'phone' signed in"

must "$OWNER" POST /cli/zen/setup '{}'

# A workspace that is an agent; sets WS_<NAME> (its project id).
seed_ws() {
  local name="$1" dir id
  dir="$(cd "$H/$name" && pwd -P)"
  must "$OWNER" POST /cli/projects/add-without-git "{\"path\":\"$dir\",\"seedWiki\":false,\"seedAgentsMd\":false}"
  id="$(jq -r '.id // .project.id // empty' <<<"$BODY")"
  must "$OWNER" POST /cli/mode '' "project=$(uri "$dir")&set=custom"
  if [[ -z "$id" ]]; then
    must "$OWNER" GET /cli/projects/list
    id="$(jq -r --arg p "$dir" '[.. | objects | select(.path? == $p) | .id] | first // empty' <<<"$BODY")"
  fi
  [[ -n "$id" ]] || die "no project id for $dir: $BODY"
  printf -v "WS_${name}" '%s' "$id"
  printf -v "DIR_${name}" '%s' "$dir"
  # A pinned Chat to carry K2's cards: the first wake of a never-started
  # agent fails (no harness on this box) and pins one.
  must "$OWNER" POST /cli/workspace/msg '' "workspace=$name&text=$(uri hello)&from=smoke&wake=true&wake_timeout=2"
  must "$OWNER" GET /cli/thread '' "addr=$name&limit=0"
  [[ "$(jq -r '.conversation_id // empty' <<<"$BODY")" != "" ]] || die "no pinned Chat for $name: $BODY"
}

# A live agent session in workspace NAME; sets PASS_<NAME> (its passport).
agent_session() {
  local name="$1" dir="DIR_$1" tf="$ROOT/token-$1"
  must "$OWNER" POST /cli/sessions/v2/spawn "$(jq -nc --arg n "tab-ga-smoke-$name" --arg c "${!dir}" --arg cmd claude --arg tf "$tf" \
    '{agent_name: $n, cwd: $c, command: $cmd, args: [], env: {K2_GA_SMOKE_TOKEN_FILE: $tf}}')"
  for _ in $(seq 1 100); do [[ -s "$tf" ]] && break; sleep 0.1; done
  [[ -s "$tf" ]] || die "the fake agent in $name never wrote its passport (spawn: $BODY)"
  printf -v "PASS_${name}" '%s' "$(cat "$tf")"
}

seed_ws alpha
seed_ws bravo
# The fake harness: the daemon resolves every `claude` through
# K2_TEST_AGENT_SHIM_DIR, so it goes there AFTER the wakes above pinned the
# Chats (with no shim, a wake fails and pins one). It writes its passport
# when asked to (our sessions), else just sleeps (a later wake).
cat >"$H/agent-shim/claude" <<'EOF'
#!/bin/sh
if [ -n "${K2_GA_SMOKE_TOKEN_FILE:-}" ]; then
  printf '%s' "$K2_HOOK_TOKEN" > "$K2_GA_SMOKE_TOKEN_FILE.tmp" && mv "$K2_GA_SMOKE_TOKEN_FILE.tmp" "$K2_GA_SMOKE_TOKEN_FILE"
fi
exec sleep 3600
EOF
chmod +x "$H/agent-shim/claude"
agent_session alpha
agent_session bravo
say "agents: alpha=$WS_alpha bravo=$WS_bravo"

# ── 0. the switch ────────────────────────────────────────────────────
say "step 0: the switch"
call "$OWNER" GET /cli/settings/get
want "settings/get carries gardenAccess=false (default off)" 200 '.gardenAccess == false'
call "$OWNER" POST /cli/zen/widget/new '{"name":"early"}'
want "off: the person makes widget 'early' (stamped as at 0.46.3)" 200
call "$OWNER" GET /cli/zen/garden/access '' "garden=x"
want "off: garden/access is an unknown route" 404
call "$OWNER" POST /cli/zen/garden/access '{"garden":"x","agent":"y","allow":true}'
want "off: a POST to garden/access is 0.46.3's 405" 405
[[ ! -e "$H/.k2/garden-access-on" ]] && ok "off: no CLI switch mark" || bad "off: ~/.k2/garden-access-on exists"
call "$PASS_alpha" POST /cli/zen/garden/new '{"name":"Off garden"}'
want "off: an agent's garden/new is 0.46.3's refusal" 403 '.error == "zen_local_only"'
call "$LOGIN" POST /cli/settings/update '{"gardenAccess":true}'
want "an Owner login turns the switch on" 200 '.gardenAccess == true'
call "$PASS_alpha" POST /cli/settings/update '{"gardenAccess":false}'
[[ "$CODE" == 4?? ]] && ok "an agent can't flip the switch ($CODE)" || bad "an agent flipped the switch: $CODE $BODY"
call "$OWNER" GET /cli/settings/get
want "the switch is on" 200 '.gardenAccess == true'
[[ -e "$H/.k2/garden-access-on" ]] && ok "on: the CLI switch mark exists" || bad "on: no ~/.k2/garden-access-on"

# ── 1. alpha makes a Garden ──────────────────────────────────────────
say "step 1: alpha makes a Garden"
manifest() { jq -nc '{schema: 1, name: "Tally", entry: "index.html", caps: ["store:read", "store:write", "thread:post", "agents:read", "files:read"]}'; }
page() {
  printf 'schema = 1\n\n[layout]\nkind = "columns"\n[[layout.column]]\nsize = 100\nmin-width = %s\n\n[[widget]]\nid = "t1"\nkind = "custom"\nwidget = "tally"\ncolumn = 0\n' "$1"
}
files_json() { jq -nc --arg m "$(manifest)" --arg h "$1" '[{name: "manifest.json", text: $m}, {name: "index.html", text: $h}]'; }
NEW="$(jq -nc --arg p "$(page 200)" --argjson f "$(files_json '<!doctype html><p>tally 1</p>')" \
  '{name: "Sales board", page: $p, widgets: [{widget: "tally", files: $f}]}')"
call "$PASS_alpha" POST /cli/zen/garden/new "$NEW"
keep garden_new
want "alpha makes 'Sales board' with its page and widget, no card needed" 200 '.made == true and (.garden.id | type == "string")'
G="$(jq -r '.garden.id' <<<"$BODY")"
CARD1="$(jq -r '.card.card // empty' <<<"$BODY")"
[[ -n "$CARD1" ]] && ok "the garden.access card is in the same answer ($CARD1)" || bad "no card in garden/new: $BODY"
call "$OWNER" GET /cli/zen/garden/access '' "garden=$G"
keep garden_access_get
want "the person reads the list: empty, made by alpha" 200 \
  "(.editors | length) == 0 and .made.agent == \"$WS_alpha\" and .sees == \"everything\""
call "$PASS_alpha" GET /cli/zen/garden/access '' "garden=$G"
want "alpha reads only whether it is listed" 200 '.listed == false and (has("editors") | not)'
call "$OWNER" GET /cli/zen/gardens
keep gardens
want "zen/gardens rows carry editors and madeBy" 200 \
  "(.gardens[] | select(.id == \"$G\") | .editors == 0 and .madeBy.agent == \"$WS_alpha\")"
call "$OWNER" GET /cli/zen/get '' "garden=$G"
want "before Allow, alpha's widget runs with Always only (signed:false)" 200 \
  '[.. | objects | select(.id? == "t1" and .kind? == "custom")] | first | .signed == false'
call "$PASS_alpha" POST /cli/zen/garden/rename "{\"garden\":\"$G\",\"name\":\"Sales\"}"
want "alpha's next edit needs the card (403 garden_access_needed)" 403 ".error == \"garden_access_needed\" and .garden == \"$G\""

# ── 2. Allow on the card ─────────────────────────────────────────────
say "step 2: the person allows"
call "$OWNER" GET /cli/thread '' "addr=alpha&cards=2&limit=50"
keep thread_alpha_cards2
want "the card is on alpha's Thread as a cards=2 permission view" 200 \
  "[.items[] | select(.id == \"$CARD1\") | .doc.choice.action.view] | first | .v == 1 and .kind == \"permission\" and .action == \"garden.access\" and .state == \"pending\""
call "$OWNER" POST /cli/thread/answer "$(jq -nc --arg c "$CARD1" '{addr: "alpha", id: $c, answer: "Allow"}')"
keep card_allow
want "Allow on the card" 200
call "$OWNER" GET /cli/zen/garden/access '' "garden=$G"
want "alpha is listed, added by the card" 200 \
  "[.editors[] | select(.agent == \"$WS_alpha\")] | first | .addedBy == \"card\""
call "$OWNER" GET /cli/zen/get '' "garden=$G"
want "after Allow, alpha's widget gains the Garden's data with no re-sign" 200 \
  '[.. | objects | select(.id? == "t1" and .kind? == "custom")] | first | .signed == true'

# ── 3. alpha edits ───────────────────────────────────────────────────
say "step 3: alpha edits"
call "$PASS_alpha" POST /cli/zen/garden/rename "{\"garden\":\"$G\",\"name\":\"Sales\"}"
want "alpha renames it" 200
call "$PASS_alpha" POST /cli/zen/garden/put "$(jq -nc --arg g "$G" --arg t "$(page 240)" '{garden: $g, text: $t, note: "wider"}')"
want "alpha puts its page" 200 '.changed == true'
call "$PASS_alpha" POST /cli/zen/widget/folder/put "$(jq -nc --argjson f "$(files_json '<!doctype html><p>tally 2</p>')" '{widget: "tally", files: $f}')"
keep widget_folder_put
want "alpha puts its widget folder, stamped and vouched" 200 '.signed.madeBy != null and .signed.vouched == true'
call "$OWNER" GET /cli/zen/widgets
keep widgets
want "zen/widgets rows carry signedBy, changedAt and placements[].signed" 200 \
  "[.widgets[] | select(.name == \"tally\" or .widget == \"tally\")] | first | (.signedBy.kind == \"agent\") and (.changedAt | type == \"number\") and ([.placements[] | select(.garden == \"$G\" or .gardenId == \"$G\") | .signed] | first == true)"

# ── 4. bravo: refused to edit, still used ───────────────────────────
say "step 4: bravo"
call "$PASS_bravo" POST /cli/zen/garden/put "$(jq -nc --arg g "$G" --arg t "$(page 260)" '{garden: $g, text: $t}')"
keep refused_bravo
want "unlisted bravo is refused with a card" 403 \
  ".error == \"garden_access_needed\" and .garden == \"$G\" and (.card.card | type == \"string\")"
call "$OWNER" GET /cli/zen/get '' "garden=$G"
HASH="$(jq -r '[.. | objects | select(.id? == "t1" and .kind? == "custom")] | first | .hash // empty' <<<"$BODY")"
[[ -n "$HASH" ]] || bad "no live hash for t1 in the page: $BODY"
ORIGIN="$(jq -nc --arg g "$G" --arg h "$HASH" '{widget: "tally", garden: $g, placement: "t1", hash: $h}')"
call "$OWNER" POST /cli/thread/post "$(jq -nc --arg o "$ORIGIN" '{addr: "bravo", text: "from the Sales tally", origin: $o, via: "compose"}')"
want "use is never gated: the Garden's widget posts to unlisted bravo's Thread" 200
call "$OWNER" GET /cli/thread '' "addr=bravo&limit=20"
want "bravo's Thread has the widget's post" 200 '[.items[] | tostring | select(test("from the Sales tally"))] | length >= 1'

# ── 5. lock, design updates ─────────────────────────────────────────
say "step 5: lock and design updates"
call "$OWNER" POST /cli/zen/garden/lock "{\"garden\":\"$G\",\"locked\":true}"
want "the person locks it" 200 '.locked == true'
call "$PASS_alpha" POST /cli/zen/garden/put "$(jq -nc --arg g "$G" --arg t "$(page 280)" '{garden: $g, text: $t}')"
want "a locked Garden refuses listed alpha (423 first)" 423
LOCKED_TEXT="$(cat "$H/.k2/zen/gardens/$G.toml")"
printf '%s\n# a hand edit while locked\n' "$LOCKED_TEXT" >"$H/.k2/zen/gardens/$G.toml"
sleep 1.5 # the watcher puts it back
[[ "$(cat "$H/.k2/zen/gardens/$G.toml")" == "$LOCKED_TEXT" ]] && ok "a hand edit to a locked Garden is put back" ||
  bad "the locked page was not put back: $(cat "$H/.k2/zen/gardens/$G.toml")"
call "$OWNER" POST /cli/zen/garden/lock "{\"garden\":\"$G\",\"locked\":false}"
want "the person unlocks it" 200 '.locked == false'
call "$OWNER" POST /cli/zen/garden/sync "{\"garden\":\"$G\",\"part\":\"both\",\"sync\":false}"
want "the person turns design updates off" 200

# ── 6. delete: ask, phone allows, restore ───────────────────────────
say "step 6: delete and restore (the phone answers)"
call "$PASS_alpha" POST /cli/zen/garden/delete "{\"garden\":\"$G\"}"
keep delete_person
want "alpha can't delete: 403 garden_delete_person with the garden.delete card" 403 \
  '.error == "garden_delete_person" and (.card.card | type == "string")'
CARD2="$(jq -r '.card.card // empty' <<<"$BODY")"
# The phone: an Owner login reads the Thread with cards=2.
call "$LOGIN" GET /cli/thread '' "addr=alpha&cards=2&limit=50"
keep phone_thread_cards2
want "phone: the delete card is a standard permission view (v1, Allow/Deny, click, can answer)" 200 \
  "[.items[] | select(.id == \"$CARD2\") | .doc.choice] | first | (.action.view.v == 1) and (.action.view.kind == \"permission\") and (.action.view.action == \"garden.delete\") and (.action.view.proof.kind == \"click\") and (.action.view.viewer.canAnswer == true) and ([.options[].label] == [\"Allow\", \"Deny\"])"
call "$LOGIN" GET /cli/thread '' "addr=alpha&limit=50"
want "phone: without cards=2 the card still reads as text" 200 \
  "[.items[] | select(.id == \"$CARD2\")] | first | (.doc.choice.prompt | type == \"string\")"
call "$LOGIN" POST /cli/thread/answer "$(jq -nc --arg c "$CARD2" '{addr: "alpha", id: $c, answer: "Allow"}')"
keep phone_delete_allow
want "phone: the Owner login answers Allow on the delete card" 200
call "$OWNER" GET /cli/zen/gardens
want "the Garden is gone" 200 "[.gardens[] | select(.id == \"$G\")] | length == 0"
call "$OWNER" GET /cli/zen/gardens/deleted
keep gardens_deleted
want "it is listed in Deleted Gardens" 200 \
  "[.gardens[] | select(.id == \"$G\")] | first | (.deletedAt | type == \"number\") and (.name == \"Sales\")"
call "$OWNER" GET /cli/zen/garden/history '' "garden=$G&limit=200"
want "a deleted Garden's History still answers" 200 '[.items[].kind] | index("deleted") != null'
call "$OWNER" POST /cli/zen/garden/restore "{\"garden\":\"$G\"}"
keep garden_restore
want "restore puts it back with its editors" 200 ".garden.id == \"$G\" and .editorsRestored >= 1"
call "$OWNER" GET /cli/zen/garden/access '' "garden=$G"
want "alpha is listed again after restore" 200 "[.editors[] | select(.agent == \"$WS_alpha\")] | length == 1"

# ── 7. Sign ─────────────────────────────────────────────────────────
say "step 7: a raw edit, then Sign"
printf '<!doctype html><p>tally raw</p>\n' >"$H/.k2/zen/widgets/tally/index.html"
sleep 1.5 # the watcher's debounce
call "$OWNER" GET /cli/zen/widgets
want "the raw edit is the live version" 200
call "$OWNER" GET /cli/zen/garden/history '' "garden=$G&limit=200"
keep garden_history_unsigned
want "History lists tally as not signed" 200 '[.unsigned[].widget] | index("tally") != null'
call "$OWNER" GET /cli/zen/widget/history '' "widget=tally&limit=200"
keep widget_history_unsigned
want "the widget's History says not signed for this Garden" 200 \
  "[.unsigned[] | select(.widget == \"tally\") | .gardens[]] | index(\"$G\") != null"
call "$PASS_bravo" POST /cli/zen/widget/sign "{\"widget\":\"tally\",\"garden\":\"$G\"}"
call "$OWNER" GET /cli/zen/garden/history '' "garden=$G&limit=200"
want "an unlisted agent's sign doesn't clear it" 200 '[.unsigned[].widget] | index("tally") != null'
call "$OWNER" POST /cli/zen/widget/sign "{\"widget\":\"tally\",\"garden\":\"$G\"}"
keep widget_sign
want "the person signs" 200 '.widget == "tally" and (.hash | type == "string")'
call "$OWNER" GET /cli/zen/garden/history '' "garden=$G&limit=200"
want "nothing is unsigned after Sign" 200 '(.unsigned | length) == 0'
call "$LOGIN" POST /cli/zen/widget/sign "{\"widget\":\"tally\",\"garden\":\"$G\"}"
want "an Owner login may sign too (G14)" 200

# ── 8. History ──────────────────────────────────────────────────────
say "step 8: History shows every step"
call "$OWNER" GET /cli/zen/garden/history '' "garden=$G&limit=200"
keep garden_history
if [[ "$CODE" == 200 ]]; then
  for k in made access_asked access_added renamed changed widget_changed locked unlocked lock_put_back sync delete_asked deleted restored disk_edit signed; do
    if jq -e --arg k "$k" '[.items[].kind] | index($k) != null' >/dev/null <<<"$BODY"; then ok "History has '$k'"; else bad "History has no '$k': $(jq -c '[.items[] | [.kind, .actor.kind]]' <<<"$BODY")"; fi
  done
  jq -e '[.items[].at] as $a | $a == ($a | sort | reverse)' >/dev/null <<<"$BODY" && ok "History is newest first" || bad "History order: $(jq -c '[.items[].at]' <<<"$BODY")"
  jq -e '[.items[].id] | all(test("^(event|snapshot|stamp|proposal):"))' >/dev/null <<<"$BODY" &&
    ok "every History id names its source" || bad "History ids: $(jq -c '[.items[].id]' <<<"$BODY")"
  jq -e "[.items[] | select(.kind == \"access_added\") | .actor.kind] | index(\"owner\") != null or index(\"login\") != null" >/dev/null <<<"$BODY" &&
    ok "the card's Allow is the person's line" || bad "access_added actor: $(jq -c '[.items[] | select(.kind == "access_added")]' <<<"$BODY")"
  jq -e '[.items[] | select(.kind == "deleted") | .actor.kind] | first == "login"' >/dev/null <<<"$BODY" &&
    ok "the phone's delete is the login's line" || bad "deleted actor: $(jq -c '[.items[] | select(.kind == "deleted")]' <<<"$BODY")"
  jq -e '[.items[] | select(.kind == "disk_edit") | .actor.kind] | first == "disk"' >/dev/null <<<"$BODY" &&
    ok "the raw edit is an 'outside K2' line" || bad "disk_edit actor: $(jq -c '[.items[] | select(.kind == "disk_edit")]' <<<"$BODY")"
  jq -e '[.items[] | select(.kind == "lock_put_back") | .actor.kind] | first == "disk"' >/dev/null <<<"$BODY" &&
    ok "the put-back is an 'outside K2' line" || bad "lock_put_back actor: $(jq -c '[.items[] | select(.kind == "lock_put_back")]' <<<"$BODY")"
  jq -e '[.items[] | select(.kind == "changed" and .snapshot != null) | .snapshot.file] | first | test("^gardens/")' >/dev/null <<<"$BODY" &&
    ok "a page change links its kept version (View this version)" || bad "no changed line with a snapshot: $(jq -c '[.items[] | select(.kind == "changed")]' <<<"$BODY")"
else
  bad "garden/history: HTTP $CODE: $BODY"
fi
# The merge's older sources: Garden 1 has kept versions from before the
# switch (no event covers them), and 'early' was stamped while it was off.
call "$OWNER" GET /cli/zen/gardens
G1="$(jq -r '.gardens[0].id // empty' <<<"$BODY")"
call "$OWNER" GET /cli/zen/garden/history '' "garden=$G1&limit=200"
keep garden1_history
want "Garden 1's History shows its kept versions (source snapshot)" 200 \
  '[.items[] | select(.source == "snapshot")] | length >= 1 and (map(.snapshot.file) | all(type == "string"))'
call "$OWNER" GET /cli/zen/widget/history '' "widget=early&limit=200"
keep widget_history_stamp
want "a stamp from before the switch is a 'signed' line (source stamp)" 200 \
  '[.items[] | select(.source == "stamp" and .kind == "signed")] | length >= 1'
call "$OWNER" GET /cli/zen/garden/history '' "garden=$G&kind=access"
want "the Access filter keeps only access lines" 200 '[.items[].kind] | all(. == "access_added" or . == "access_removed")'
call "$OWNER" GET /cli/zen/garden/history '' "garden=$G&limit=2"
want "paging: 2 lines and a next cursor" 200 '(.items | length) == 2 and (.next | type == "string")'
NEXT="$(jq -r '.next // empty' <<<"$BODY")"
call "$OWNER" GET /cli/zen/garden/history '' "garden=$G&limit=2&before=$(uri "$NEXT")"
want "the next page answers" 200 '(.items | length) >= 1'
call "$PASS_alpha" GET /cli/zen/garden/history '' "garden=$G"
[[ "$CODE" == 403 ]] && ok "an agent can't read History (403)" || bad "agent History read: $CODE $BODY"
call "$OWNER" GET /cli/zen/history '' "garden=$G"
SNAP="$(jq -r '[.. | objects | select(.name? and (.name | test("\\.toml$")))] | first | .name // empty' <<<"$BODY")"
if [[ -n "$SNAP" ]]; then
  call "$OWNER" GET /cli/zen/snapshot '' "file=$(uri "gardens/$G.toml")&name=$(uri "$SNAP")"
  keep snapshot_garden
  want "View this version (a Garden page)" 200 '(.text | type == "string") and (.text | test("schema"))'
else
  bad "no Garden snapshot in zen/history: $BODY"
fi
call "$OWNER" GET /cli/zen/history '' "widget=tally"
WSNAP="$(jq -r '[.. | objects | select(.name? and (.hash? != null))] | first | .name // empty' <<<"$BODY")"
if [[ -n "$WSNAP" ]]; then
  call "$OWNER" GET /cli/zen/snapshot '' "file=$(uri widgets/tally)&name=$(uri "$WSNAP")"
  keep snapshot_widget
  want "View this version (a widget)" 200 '(.files | length) >= 1'
else
  bad "no widget snapshot in zen/history --widget: $BODY"
fi

# ── 9. switch off ───────────────────────────────────────────────────
say "step 9: switch off"
call "$OWNER" POST /cli/settings/update '{"gardenAccess":false}'
want "the person turns the switch off" 200 '.gardenAccess == false'
[[ ! -e "$H/.k2/garden-access-on" ]] && ok "off again: the CLI switch mark is gone" || bad "off: ~/.k2/garden-access-on stayed"
for r in /cli/zen/garden/access /cli/zen/garden/history /cli/zen/widget/history /cli/zen/gardens/deleted /cli/zen/snapshot; do
  call "$OWNER" GET "$r" '' "garden=$G&widget=tally"
  want "off: $r is unknown (404)" 404
done
call "$PASS_alpha" POST /cli/zen/garden/new '{"name":"Off again"}'
want "off: an agent's garden/new is refused as at 0.46.3" 403 '.error == "zen_local_only"'
call "$OWNER" GET /cli/zen/gardens
want "off: zen/gardens rows carry no editors or madeBy" 200 '[.gardens[] | has("editors") or has("madeBy")] | any | not'
call "$OWNER" GET /cli/zen/widgets
want "off: zen/widgets rows carry no signedBy" 200 '[.widgets[] | has("signedBy")] | any | not'

# ── result ──────────────────────────────────────────────────────────
if [[ "${GA_SMOKE_DUMP:-0}" == 1 ]]; then
  for f in "$FIX"/*.json; do
    printf '===FIXTURE %s===\n' "$(basename "$f" .json)"
    cat "$f"
  done
fi
say "$pass passed, $fail failed"
if ((fail > 0)); then
  printf '  - %s\n' "${FAILED[@]}" >&2
  say "daemon.log (last 40 lines):"
  tail -40 "$H/daemon.log" >&2
  exit 1
fi
say "PASS"
