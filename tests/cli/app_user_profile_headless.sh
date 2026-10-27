#!/usr/bin/env bash
# App user profile (prd-app-user-profile-v1, 0.46.5), headless: a real
# k2-daemon from this worktree's target/ under a throwaway HOME, driven with
# curl the way a phone app or an App's server calls (Authorization: Bearer).
#
#   profile.get / update / setImage, sessions (list, sign out others),
#   the owner's view, the wrong-current-password throttle, a used reset that
#   clears the login lock, and the reset door's own per-client budget.
#
# Build first (worktree target/, never main's): cargo build -p k2-daemon
# Never touches ~/.k2, the keychain or a running daemon. Fails loudly.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done
# shellcheck source=_sandbox_daemon.sh
source "$SCRIPT_DIR/_sandbox_daemon.sh"
sandbox_daemon_start

BASE="http://127.0.0.1:${K2SO_PORT}"
OWNER="$K2SO_TOKEN"
pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }
expect() { # label got want
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got $2, want $3)"; fi
}
# req METHOD PATH [BEARER] [JSON] [extra header] → sets CODE and BODY
req() {
    local m="$1" p="$2" bearer="${3:-}" data="${4:-}" extra="${5:-}" out
    local args=(-sS -o "$SANDBOX_HOME/body" -w '%{http_code}' -X "$m")
    [ -n "$bearer" ] && args+=(-H "Authorization: Bearer $bearer")
    [ -n "$extra" ] && args+=(-H "$extra")
    if [ -n "$data" ]; then args+=(-H 'Content-Type: application/json' --data "$data"); fi
    out="$(curl "${args[@]}" "$BASE$p")"
    CODE="$out"
    BODY="$(cat "$SANDBOX_HOME/body")"
}
owner() { req "$1" "$2?token=$OWNER" "" "${3:-}"; [ "$CODE" = 200 ] || { echo "owner $1 $2: $CODE $BODY" >&2; exit 1; }; }
field() { python3 -c 'import json,sys; v=json.loads(sys.argv[1]);
for k in sys.argv[2].split("."):
    v = v[int(k)] if isinstance(v, list) else v.get(k)
print(json.dumps(v) if not isinstance(v, str) else v)' "$BODY" "$1"; }

echo "== setup: a box-login guest with a password and an email"
owner POST /cli/skin/users '{"username":"dana"}'
owner POST /cli/skin/users/password '{"username":"dana","password":"s3cret-horse"}'
owner POST /cli/skin/users/email '{"username":"dana","email":"dana@example.test"}'
owner POST /cli/skin/roles '{"name":"staff"}'
owner POST /cli/skin/roles/assign '{"username":"dana","role":"staff"}'
login() { req POST /cli/skin/login "" "{\"username\":\"dana\",\"password\":\"$1\"}" "${2:-}"; }
login s3cret-horse 'User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15'
expect "login A" "$CODE" 200
A="$(field token)"
login s3cret-horse
B="$(field token)"

echo "== profile.get"
req GET /cli/skin/profile "$A"
expect "profile.get 200" "$CODE" 200
expect "username" "$(field username)" dana
expect "no picture unless asked" "$(field image)" null
expect "two sessions" "$(field counts.sessions)" 2
req GET "/cli/skin/profile?token=$OWNER"
expect "owner token is not a person" "$CODE" 403

echo "== profile.update"
req POST /cli/skin/profile "$A" '{"fullName":"Dana ‮Ruiz","displayName":"Dee"}'
expect "update 200" "$CODE" 200
expect "name cleaned" "$(field profile.fullName)" "Dana Ruiz"
expect "self-set mark" "$(field profile.nameSetBySelf)" true

echo "== profile.setImage"
PNG='data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='
req POST /cli/skin/profile/image "$A" "{\"image\":\"$PNG\"}"
expect "setImage 200" "$CODE" 200
case "$(field image)" in data:image/jpeg\;base64,*) ok "stored as K2's JPEG" ;; *) bad "stored as K2's JPEG ($BODY)" ;; esac
req POST /cli/skin/profile/image "$A" '{"image":"data:image/svg+xml;base64,PHN2Zy8+"}'
expect "svg refused" "$CODE" 415

echo "== sessions"
req GET /cli/skin/sessions "$A"
expect "sessions 200" "$CODE" 200
expect "current first" "$(field sessions.0.current)" true
expect "coarse client label" "$(field sessions.0.client)" "Safari on macOS"
case "$(field sessions.0.id)" in ses_*) ok "opaque id" ;; *) bad "opaque id ($BODY)" ;; esac
req POST /cli/skin/sessions/revoke-others "$A"
expect "revoke-others ended 1" "$(field ended)" 1
req GET /cli/skin/profile "$B"
expect "B ended" "$CODE" 401
req GET /cli/skin/profile "$A"
expect "A stays" "$CODE" 200
req GET "/cli/skin/access/profile?token=$OWNER&app=host&user=dana"
expect "owner view" "$(field nameSetBySelf)" true

echo "== password.change: wrong current passwords"
for i in 1 2 3 4 5; do
    req POST /cli/skin/password/change "$A" '{"oldPassword":"nope-nope","password":"next-pass-1"}'
    expect "wrong $i → 401" "$CODE" 401
done
req POST /cli/skin/password/change "$A" '{"oldPassword":"s3cret-horse","password":"next-pass-1"}'
expect "6th → 429" "$CODE" 429
expect "rate_limited" "$(field error.code)" rate_limited
login s3cret-horse
expect "never the login lock" "$CODE" 200

echo "== a used reset clears the login lock"
for i in 1 2 3; do login wrong-wrong-1; done
login s3cret-horse
expect "locked" "$CODE" 423
req POST "/cli/skin/password/forgot?token=$OWNER" "" '{"username":"dana"}'
expect "owner forgot" "$CODE" 200
TOK="$(field token)"
req POST /cli/skin/password/reset "" "{\"token\":\"$TOK\",\"password\":\"after-reset-1\"}"
expect "reset 200" "$CODE" 200
login after-reset-1
expect "unlocked by the reset" "$CODE" 200
req POST /cli/skin/password/reset "" "{\"token\":\"$TOK\",\"password\":\"again-again-1\"}"
expect "single use" "$CODE" 401

echo "== the reset door's own per-client budget"
for i in $(seq 1 10); do
    req POST /cli/skin/password/reset "" "{\"token\":\"k2inv_junk$i\",\"password\":\"whatever-pw-1\"}" 'X-Forwarded-For: 198.51.100.77'
done
expect "10th junk → 401" "$CODE" 401
req POST /cli/skin/password/reset "" '{"token":"k2inv_junk","password":"whatever-pw-1"}' 'X-Forwarded-For: 198.51.100.77'
expect "11th → 429" "$CODE" 429
req POST /cli/skin/password/reset "" '{"token":"k2inv_junk","password":"whatever-pw-1"}' 'X-Forwarded-For: 198.51.100.78'
expect "another client → 401" "$CODE" 401

echo
echo "app_user_profile_headless: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
