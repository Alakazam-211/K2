#!/usr/bin/env bash
# Gardens on the phone: the remote Garden door, headless smoke
# (prd-gardens-on-phone-v1 §9 "Smoke"; contract-gardens-phone-v1).
#
# Builds this checkout's k2-daemon, runs it from `env -i` in a temp HOME
# (every K2_*/K2SO_* dropped; never the real ~/.k2, the keychain, the
# running daemon or the tunnel) and drives it with curl only, as K2
# Companion would: pair a fake phone through the real pairing routes, walk
# every /cli/zen/remote/* route and its refusals, then revoke the phone.
#
#   scripts/test/gardens_remote_smoke.sh      (KEEP=1 keeps the temp dir;
#                                              GR_SMOKE_NO_BUILD=1 skips the build)
#
# Steps (each check is PASS or FAIL; any FAIL exits 1 at the end):
#   0  /boot-status lists gardens-remote-v1
#   1  Zen set up; a widget `board` placed as `b1` on the first Garden
#   2  logins: the owner's linked Admin login paired as a phone; another
#      person's unlinked Owner login paired too; the linked login's password
#      session
#   3  refusals on every route: another person → 403 garden_person_only;
#      the owner on a password login → 403 garden_device_only; a GET on a
#      POST route → 405
#   4  the owner's phone: feed (full, then unchanged), page (no sources,
#      remote.widgets), frame (sealed document + hello), call (store.put,
#      store.get, a stale hash, a later verb), blob put/get, report, pause
#      and Resume, agents order
#   5  a mobile layout (contract rev 2): a bad one refused, a good one on the
#      page; the hello's merged config, view and height; frameVersion moves on a
#      config edit and not on a label; saves apart from the desktop's
#   6  revoke the phone: the next request is 403 device_session_ended
set -uo pipefail
for t in curl jq cargo; do command -v "$t" >/dev/null || { echo "[gr-smoke] FAIL: needs $t" >&2; exit 1; }; done
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
say() { printf '[gr-smoke %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

if [[ "${GR_SMOKE_NO_BUILD:-0}" != 1 ]]; then
  say "building k2-daemon"
  (cd "$REPO" && cargo build -q -p k2-daemon --bin k2-daemon) || { say "FAIL: build"; exit 1; }
fi
BIN="$(cd "$REPO" && cargo metadata -q --format-version 1 --no-deps | jq -r .target_directory)/debug/k2-daemon"
[[ -x "$BIN" ]] || { say "FAIL: no daemon binary at $BIN"; exit 1; }

ROOT="$(mktemp -d /tmp/k2gr-XXXX)"
H="$ROOT/h"
PATH_CLEAN="/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin"
mkdir -p "$H/.k2" "$H/tmp" "$H/run" "$H/agent-shim"
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
# The phone sends its device session as a Bearer, as the contract says.
call() {
  local tok="$1" method="$2" path="$3" body="${4:-}" query="${5:-}" out
  out="$(curl -sS -X "$method" "$U$path${query:+?$query}" -H "Authorization: Bearer $tok" -H 'X-K2-Client: phone' \
    -H 'content-type: application/json' ${body:+--data "$body"} -w $'\n%{http_code}')" || out=$'curl failed\n000'
  CODE="${out##*$'\n'}"
  BODY="${out%$'\n'*}"
}
must() {
  call "$@"
  [[ "$CODE" == 2?? ]] || die "$2 $3 → $CODE: $BODY"
}
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

# ── 0 ────────────────────────────────────────────────────────────────
say "step 0: the feature key"
BOOT="$(curl -fsS "$U/boot-status")"
if jq -e '.features | index("gardens-remote-v1")' >/dev/null <<<"$BOOT"; then ok "boot-status lists gardens-remote-v1"; else bad "boot-status features: $BOOT"; fi

# ── 1 ────────────────────────────────────────────────────────────────
say "step 1: a Garden with a custom widget"
must "$OWNER" POST /cli/zen/setup '{}'
GARDEN="$(jq -r '.gardens[0].id' <<<"$BODY")"
[[ "$GARDEN" == g-* ]] || die "no first Garden: $BODY"
must "$OWNER" POST /cli/zen/widget/new '{"name":"board","from":"hello"}'
cat >"$H/.k2/zen/gardens/$GARDEN.toml" <<'EOF'
schema = 1

[layout]
kind = "columns"
[[layout.column]]
size = 100
min-width = 200

[[widget]]
id = "b1"
kind = "custom"
widget = "board"
column = 0
EOF
must "$OWNER" POST /cli/zen/reload '{}'

# ── 2 ────────────────────────────────────────────────────────────────
say "step 2: logins and paired phones"
PW="correct-horse-battery-staple-9"
must "$OWNER" POST /cli/users/add "{\"username\":\"rosphone\",\"password\":\"$PW\",\"mustChangePassword\":false}"
must "$OWNER" POST /cli/users/set-role '{"username":"rosphone","role":"admin"}'
must "$OWNER" POST /cli/owner/accounts '{"accounts":["rosphone"]}'
must "$OWNER" POST /cli/users/add "{\"username\":\"other\",\"password\":\"$PW\",\"mustChangePassword\":false}"
must "$OWNER" POST /cli/users/set-role '{"username":"other","role":"owner"}'
# pair ASUSER DEVICE → prints the device session (the owner token mints the code).
pair() {
  local user="$1" device="$2" code
  if [[ -n "$user" ]]; then must "$OWNER" POST /cli/auth/device/pair-code "{\"user\":\"$user\"}"; else must "$OWNER" POST /cli/auth/device/pair-code '{}'; fi
  code="$(jq -r '.code' <<<"$BODY")"
  [[ -n "$code" && "$code" != null ]] || die "no pairing code: $BODY"
  BODY="$(curl -sS -X POST "$U/cli/auth/device/pair" -H 'content-type: application/json' \
    --data "{\"code\":\"$code\",\"device\":{\"id\":\"$device\",\"name\":\"Smoke phone\"}}")"
  jq -r '.token' <<<"$BODY"
}
PHONE="$(pair "" ios-smoke-1)"
[[ -n "$PHONE" && "$PHONE" != null ]] || die "the owner's phone didn't pair: $BODY"
OTHER="$(pair other android-smoke-2)"
[[ -n "$OTHER" && "$OTHER" != null ]] || die "the other phone didn't pair: $BODY"
must "$OWNER" POST /cli/auth/login "{\"username\":\"rosphone\",\"password\":\"$PW\",\"mustChangePassword\":false}"
PASSWORD_LOGIN="$(jq -r '.token' <<<"$BODY")"
ok "paired two phones and signed in once with a password"

# ── 3 ────────────────────────────────────────────────────────────────
say "step 3: refusals"
GETS=("/cli/zen/remote/gardens" "/cli/zen/remote/page" "/cli/zen/remote/frame" "/cli/zen/remote/agents-order")
for p in "${GETS[@]}"; do
  call "$OTHER" GET "$p" "" "garden=$GARDEN&placement=b1"
  want "another person's Owner phone: $p" 403 '.ok == false and .error.code == "garden_person_only"'
  call "$PASSWORD_LOGIN" GET "$p" "" "garden=$GARDEN&placement=b1"
  want "the owner on a password login: $p" 403 '.error.code == "garden_device_only"'
done
call "$OTHER" POST /cli/zen/remote/call "{\"garden\":\"$GARDEN\",\"placement\":\"b1\",\"hash\":\"x\",\"verb\":\"store.get\",\"args\":[\"obj/1\"]}"
want "another person can't call" 403 '.error.code == "garden_person_only"'
call "$PHONE" GET /cli/zen/remote/call
want "a GET on a POST row" 405

# ── 4 ────────────────────────────────────────────────────────────────
say "step 4: the owner's phone"
call "$PHONE" GET /cli/zen/remote/gardens
want "the feed" 200 '.ok and .person == "owner" and .setUp and .full and (.gardens | length) > 0 and (.gardens[0] | has("version"))'
CURSOR="$(jq -r '.cursor' <<<"$BODY")"
call "$PHONE" GET /cli/zen/remote/gardens "" "since=$CURSOR"
want "the feed, unchanged" 200 '.full == false and .gardens == []'
call "$PHONE" GET /cli/zen/remote/page "" "garden=$GARDEN"
want "the page" 200 '(has("sources") | not) and .page.phone == null and .remote.widgets[0].id == "b1" and .remote.widgets[0].run == "frame" and (.remote.widgets[0].frameVersion | length) > 0 and (.remote.features | index("garden-data-v1"))'
call "$PHONE" GET /cli/zen/remote/frame "" "garden=$GARDEN&placement=b1&scheme=dark"
want "the frame" 200 '(.srcdoc | contains("Content-Security-Policy")) and .hello.k2 == "hello" and (.hash | length) == 64 and .hello.view == "phone" and .hello.height == "fill" and .hello.config == {} and (.frameVersion | length) > 0'
HASH="$(jq -r '.hash' <<<"$BODY")"
call "$PHONE" POST /cli/zen/remote/call "{\"garden\":\"$GARDEN\",\"placement\":\"b1\",\"hash\":\"$HASH\",\"verb\":\"viewer.get\",\"args\":[]}"
want "call viewer.get" 200 '.ok == true'
call "$PHONE" POST /cli/zen/remote/call "{\"garden\":\"$GARDEN\",\"placement\":\"b1\",\"hash\":\"$(printf '0%.0s' {1..64})\",\"verb\":\"viewer.get\",\"args\":[]}"
want "a stale hash" 409 '.error.code == "stale_widget"'
call "$PHONE" POST /cli/zen/remote/call "{\"garden\":\"$GARDEN\",\"placement\":\"b1\",\"hash\":\"$HASH\",\"verb\":\"thread.post\",\"args\":[]}"
want "a later verb" 501 '.error.code == "verb_unavailable" and .error.feature == "zen-v1"'
call "$PHONE" POST /cli/zen/remote/report "{\"garden\":\"$GARDEN\",\"placement\":\"b1\",\"hash\":\"$HASH\",\"event\":\"ready\"}"
want "report ready" 200
call "$PHONE" POST /cli/zen/remote/pause "{\"garden\":\"$GARDEN\",\"placement\":\"b1\",\"reason\":\"runaway\"}"
want "pause" 200
call "$PHONE" POST /cli/zen/remote/resume "{\"garden\":\"$GARDEN\",\"placement\":\"b1\"}"
want "Resume from the phone" 200 '.resumed == true'
call "$PHONE" GET /cli/zen/remote/agents-order "" "garden=$GARDEN"
want "agents order (read-only)" 200 '.ok == true'

# ── 5 ────────────────────────────────────────────────────────────────
say "step 5: a mobile layout"
PAGE_TOML='schema = 1
[layout]
kind = "columns"
[[layout.column]]
size = 100
min-width = 200
[[widget]]
id = "b1"
kind = "custom"
widget = "board"
column = 0
[widget.props.config]
source = "fleet"
zoom = 1
[[widget]]
id = "b2"
kind = "custom"
widget = "board"
column = 0
'
PHONE_TOML='[[phone.screen]]
label = "Board"
canvas = "full"
[[phone.screen.widget]]
id = "b1"
height = "fill"
[phone.screen.widget.config]
zoom = 2
'
put_page() { must "$OWNER" POST /cli/zen/garden/put "$(jq -n --arg g "$GARDEN" --arg t "$1" '{garden: $g, text: $t}')"; }
call "$OWNER" POST /cli/zen/garden/put "$(jq -n --arg g "$GARDEN" --arg t "$PAGE_TOML[[phone.screen]]
[[phone.screen.widget]]
id = \"nope\"
" '{garden: $g, text: $t}')"
want "a layout naming a widget not on the page is refused" 400 '.message | contains("phone screen 1 names widget '"'"'nope'"'"', which is not on this page")'
call "$OWNER" POST /cli/zen/garden/put "$(jq -n --arg g "$GARDEN" --arg t "$PAGE_TOML[[phone.screen]]
[[phone.screen.widget]]
id = \"b1\"
height = 3
" '{garden: $g, text: $t}')"
want "a bad height is refused" 400 '.message | contains("widget height is")'
put_page "$PAGE_TOML"
call "$PHONE" GET /cli/zen/remote/frame "" "garden=$GARDEN&placement=b1&scheme=dark"
want "no layout: height from the column" 200 '.hello.height == "auto" and .hello.config == {"source":"fleet","zoom":1}'
call "$PHONE" GET /cli/zen/remote/page "" "garden=$GARDEN"
want "no layout: no page.phone" 200 '(.page | has("phone") | not)'
put_page "$PAGE_TOML$PHONE_TOML"
call "$PHONE" GET /cli/zen/remote/page "" "garden=$GARDEN"
want "page.phone is on the page" 200 '.page.phone == {"screens":[{"label":"Board","canvas":"full","widgets":[{"id":"b1","height":"fill","config":{"zoom":2}}]}]} and (.remote.widgets | map(.id)) == ["b1","b2"]'
FV1="$(jq -r '.remote.widgets[0].frameVersion' <<<"$BODY")"
call "$PHONE" GET /cli/zen/remote/frame "" "garden=$GARDEN&placement=b1&scheme=dark"
want "the hello merges config" 200 '.hello.view == "phone" and .hello.height == "fill" and .hello.config == {"source":"fleet","zoom":2} and .frameVersion == "'"$FV1"'"'
HASH="$(jq -r '.hash' <<<"$BODY")"
call "$PHONE" GET /cli/zen/remote/frame "" "garden=$GARDEN&placement=b2&scheme=dark"
want "a widget the layout leaves out still answers, with no height" 200 '(.hello | has("height") | not) and .hello.view == "phone"'
put_page "$PAGE_TOML${PHONE_TOML/label = \"Board\"/label = \"Renamed\"}"
call "$PHONE" GET /cli/zen/remote/page "" "garden=$GARDEN"
want "a label edit leaves frameVersion alone" 200 '.remote.widgets[0].frameVersion == "'"$FV1"'"'
put_page "$PAGE_TOML${PHONE_TOML/zoom = 2/zoom = 3}"
call "$PHONE" GET /cli/zen/remote/page "" "garden=$GARDEN"
want "a config edit moves frameVersion" 200 '.remote.widgets[0].frameVersion != "'"$FV1"'"'
save_body() { jq -n --arg g "$GARDEN" --arg h "$HASH" --arg v "$1" --argjson a "$2" '{garden: $g, placement: "b1", hash: $h, verb: $v, args: $a}'; }
call "$PHONE" POST /cli/zen/remote/call "$(save_body save.set '["camera", {"zoom": 3}]')"
want "a phone saves" 200 '.ok == true'
call "$PHONE" POST /cli/zen/remote/call "$(save_body save.get '["camera"]')"
want "and reads its own" 200 '.result == {"zoom": 3}'
curl -sS -X POST "$U/cli/zen/call" -H "Authorization: Bearer $OWNER" -H 'Origin: tauri://localhost' -H 'content-type: application/json' --data "$(save_body save.get '["camera"]')" >"$ROOT/desk.json"
if jq -e '.result == null' "$ROOT/desk.json" >/dev/null; then ok "the desktop's saves didn't move"; else bad "the desktop reads the phone's save: $(cat "$ROOT/desk.json")"; fi

# ── 6 ────────────────────────────────────────────────────────────────
say "step 6: revoke the phone"
must "$OWNER" GET /cli/auth/devices
DEV="$(jq -r '.devices[] | select(.deviceId == "ios-smoke-1" or .device_id == "ios-smoke-1" or .device.id == "ios-smoke-1") | .id' <<<"$BODY" | head -1)"
[[ -n "$DEV" ]] || die "the owner's phone isn't listed: $BODY"
must "$OWNER" POST /cli/auth/device/revoke "{\"id\":\"$DEV\"}"
call "$PHONE" GET /cli/zen/remote/gardens
want "a revoked phone" 403 '.error.code == "device_session_ended" or .error == "device_session_ended"'

say "done: $pass passed, $fail failed"
if ((fail > 0)); then
  for f in "${FAILED[@]}"; do say "  FAILED: $f"; done
  exit 1
fi
