#!/usr/bin/env bash
# App phone pairing, headless (prd-app-phone-pairing-v1 §11 "Headless"):
# drives start → status → check → redeem → refresh → devices → revoke with
# curl against a running K2 daemon, the way a web App and a phone app call
# it. No desktop client, no browser, no phone.
#
#   K2_URL=http://127.0.0.1:<port>      the daemon's main port (or https://<you>.k2.dev)
#   K2_APP=<service id>                 an App whose pairing is on
#   K2_PASS=k2skn_…                     a guest's fresh web session pass on that App
#   K2_FRONT=http://127.0.0.1:<port>    optional: the App's front port, to fetch
#   K2_APP_HOST=<app>.<you>.k2.dev        its link files with Host: K2_APP_HOST
#
#   bash scripts/app-pairing-smoke.sh
#
# Exits non-zero at the first answer that isn't what the contract says.
# Never prints a code, a pass or a refresh token. crates/k2-daemon/tests/
# app_pairing_integration.rs runs it against an in-process daemon.
set -euo pipefail

: "${K2_URL:?set K2_URL (the daemon main port)}"
: "${K2_APP:?set K2_APP (the service id of the App)}"
: "${K2_PASS:?set K2_PASS (a guest web session pass)}"
command -v curl >/dev/null || { echo "FAIL: curl is required" >&2; exit 2; }
# Windows (Git Bash): `python3` is usually the Microsoft Store stub. Use the
# first real Python 3 (as cli/k2 does) and drop the CRs it writes to pipes.
case "$(uname -s 2>/dev/null)" in
	MINGW*|MSYS*|CYGWIN*)
		for c in "command python3" "py -3" "python"; do
			if $c -c 'import sys; sys.exit(0 if sys.version_info[0] == 3 else 1)' >/dev/null 2>&1; then
				eval "python3() { $c \"\$@\" | tr -d '\\r'; }"
				break
			fi
		done
		;;
esac
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 2; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
pass=0
ok() { echo "  ok $*"; pass=$((pass + 1)); }
fail() { echo "FAIL: $*" >&2; exit 1; }

# call METHOD PATH [BODY] [TOKEN]: status in $st, body in $tmp/body, headers in $tmp/head.
call() {
	local method="$1" path="$2" body="${3:-}" tok="${4:-}"
	local args=(-sS -o "$tmp/body" -D "$tmp/head" -w '%{http_code}' -X "$method" -H 'User-Agent: K2PairingSmoke/1.0')
	[ -n "$tok" ] && args+=(-H "Authorization: Bearer $tok")
	[ -n "$body" ] && args+=(-H 'Content-Type: application/json' --data-binary "$body")
	st="$(curl "${args[@]}" "$K2_URL$path")"
}
field() { python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print(eval("v" + sys.argv[2]))' "$tmp/body" "$1"; }
code() { field '["error"]["code"]'; }

echo "== start (the web App, the person's own fresh pass) =="
call POST /cli/skin/pair/start '{}' "$K2_PASS"
[ "$st" = 200 ] || fail "start: $st $(cat "$tmp/body")"
grep -qi '^cache-control: no-store' "$tmp/head" || fail "start must not be cached"
the_code="$(field '["code"]')"
pid="$(field '["pairingId"]')"
url="$(field '["pairUrl"]')"
case "$the_code" in K2A-*) ;; *) fail "the code is a K2A- code" ;; esac
case "$url" in https://*/.well-known/k2-pair#"$the_code") ;; *) fail "pairUrl names the App's host and carries the code in the fragment" ;; esac
python3 -c 'import json,sys; q=json.load(open(sys.argv[1]))["qr"]; assert q["size"]==len(q["rows"]) and all(len(r)==q["size"] for r in q["rows"])' "$tmp/body" || fail "qr is square"
ok "start: a K2A code, its link on $(printf '%s' "$url" | cut -d/ -f3), a QR"
call GET /cli/skin/pair/start '' "$K2_PASS"
[ "$st" = 405 ] || fail "GET start must be 405, got $st"
ok "GET start is 405"

call GET "/cli/skin/pair/status?pairingId=$pid" '' "$K2_PASS"
[ "$st" = 200 ] && [ "$(field '["state"]')" = pending ] || fail "status pending: $st $(cat "$tmp/body")"
ok "status: pending"

if [ -n "${K2_FRONT:-}" ]; then
	echo "== the link files on the App's host =="
	st="$(curl -sS -o "$tmp/aasa" -w '%{http_code}' -H "Host: ${K2_APP_HOST:?set K2_APP_HOST with K2_FRONT}" "$K2_FRONT/.well-known/apple-app-site-association")"
	[ "$st" = 200 ] || fail "AASA on the App host: $st"
	python3 -c 'import json,sys; d=json.load(open(sys.argv[1]))["applinks"]["details"]; assert d and "/.well-known/k2-pair" in d[0]["paths"], d' "$tmp/aasa" || fail "AASA names the pair path"
	st="$(curl -sS -o "$tmp/page" -w '%{http_code}' -H "Host: $K2_APP_HOST" "$K2_FRONT/.well-known/k2-pair")"
	[ "$st" = 200 ] && grep -q "location.hash" "$tmp/page" || fail "landing page: $st"
	ok "AASA and the landing page served by K2 on $K2_APP_HOST"
fi

echo "== check and redeem (the phone app; Public, the code is the credential) =="
call POST /cli/skin/pair/check "{\"code\":\"$the_code\",\"app\":\"$K2_APP\"}"
[ "$st" = 200 ] || fail "check: $st $(cat "$tmp/body")"
who="$(field '["username"]')"
ok "check: signs in as $who (the code is not used)"
call POST /cli/skin/pair "{\"code\":\"$the_code\",\"app\":\"$K2_APP\",\"device\":{\"id\":\"smoke-phone-1\",\"name\":\"Smoke phone\"}}"
[ "$st" = 200 ] || fail "redeem: $st $(cat "$tmp/body")"
grep -qi '^cache-control: no-store' "$tmp/head" || fail "redeem must not be cached"
[ "$(field '["via"]')" = pairing ] || fail "via pairing"
[ "$(field '["username"]')" = "$who" ] || fail "the redeem signs in the person check named"
phone="$(field '["token"]')"
refresh="$(field '["refresh"]')"
case "$refresh" in k2skr_*) ;; *) fail "a refresh token" ;; esac
ok "redeem: a device sign-in (pass + refresh token)"
call POST /cli/skin/pair "{\"code\":\"$the_code\",\"app\":\"$K2_APP\",\"device\":{\"id\":\"smoke-phone-2\",\"name\":\"Again\"}}"
[ "$st" = 401 ] && [ "$(code)" = pair_code_invalid ] || fail "a second redeem: $st $(cat "$tmp/body")"
ok "the code works once"

call GET "/cli/skin/pair/status?pairingId=$pid" '' "$K2_PASS"
[ "$(field '["state"]')" = paired ] && [ "$(field '["device"]["name"]')" = "Smoke phone" ] || fail "status paired: $(cat "$tmp/body")"
ok "status: paired with Smoke phone"

echo "== the phone's sign-in is an ordinary device sign-in =="
call POST /cli/skin/refresh "{\"refresh\":\"$refresh\",\"app\":\"$K2_APP\"}"
[ "$st" = 200 ] || fail "refresh: $st $(cat "$tmp/body")"
phone="$(field '["token"]')"
refresh2="$(field '["refresh"]')"
ok "refresh rotates"
call GET /cli/skin/devices '' "$phone"
[ "$st" = 200 ] || fail "devices: $st $(cat "$tmp/body")"
dev="$(python3 -c 'import json,sys; d=[x for x in json.load(open(sys.argv[1]))["devices"] if x["deviceId"]=="smoke-phone-1"]; assert d and d[0]["via"]=="pairing", d; print(d[0]["id"])' "$tmp/body")" || fail "devices list the paired phone with via pairing"
ok "guests.devices: via pairing"
call POST /cli/skin/devices/revoke "{\"id\":\"$dev\"}" "$K2_PASS"
[ "$st" = 200 ] || fail "revoke: $st $(cat "$tmp/body")"
call POST /cli/skin/refresh "{\"refresh\":\"$refresh2\",\"app\":\"$K2_APP\"}"
[ "$st" = 401 ] || fail "a revoked phone's refresh must be refused, got $st"
ok "revoke ends it (the refresh is refused)"

echo "PASS: $pass checks"
