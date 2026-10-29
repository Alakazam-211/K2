#!/usr/bin/env bash
# Sew 0.3.1 → 0.5.0 through K2's root helper, with REAL Sew releases, AS
# ROOT INSIDE A THROWAWAY VM (never a real box). Driven by
# tests/linux/run_sew_upgrade_real.sh, which puts in /it:
#   k2-root-helper-old   a release-pins helper built at a main that pins 0.3.1
#   k2-root-helper-new   a release-pins helper built at the branch that pins 0.5.0
#   install-root-helper.sh
#   sew-0.3.1-x86_64-unknown-linux-gnu.tar.gz, sew-0.5.0-x86_64-unknown-linux-gnu.tar.gz
#     (each checked against its sew-releases SHA256SUMS before upload)
#   THROWAWAY            the marker this script refuses to run without
#
# What it proves, in K2's own order (k2-daemon/src/sew/supervisor.rs):
#   1. the OLD helper installs Sew 0.3.1 in K2 mode (stage, install-bin,
#      install, enable, start-db, migrate, start), and a member is seeded;
#   2. the NEW helper upgrades it to 0.5.0 (`engine sew upgrade`), the data
#      survives, Sew reports 0.5.0 and its 12 feature flags;
#   3. Sew 0.5.0 takes K2's contract: the managed-fields policy body, App
#      Space bind/unbind (orphan adds 422 space_orphaned), the home client
#      and App settings, and a provision that carries title/pronouns keeps
#      them (option a);
#   4. `engine sew install reconfigure` (protocol 11) runs over the 0.5.0
#      install and Sew comes back healthy;
#   5. the plain `engine sew rollback` (never `--accept-data-loss-since`) is
#      refused across the schema change (exit 3) and 0.5.0 stays healthy;
#   6. an agent writes a message on 0.5.0 (act-as on the mgmt socket);
#   7. the emergency rollback, in K2's order (Rosson 2026-10-28): a mgmt
#      backup of 0.5.0 first, then `engine sew rollback accept-data-loss
#      <the pre-upgrade bundle Sew named at upgrade>`, then 0.3.1 healthy,
#      doctor 0 fail, the pre-upgrade data back, what was written on 0.5.0
#      gone from the live chat, and the 0.5.0 backup kept.
# A fake daemon (bash, user k2, k2-daemon.service) runs every helper call as
# the daemon would (`sudo -n …` from its main process). A loopback JWKS
# (an EC P-256 key, as K2's chat-pass keyring) stands in for the daemon's
# /v1/chat/jwks. Every check fails loudly; the first failure stops the run.
set -euo pipefail

IT=/it
[ -f "$IT/THROWAWAY" ] || { echo "refusing: no $IT/THROWAWAY marker (this script installs Sew as root)" >&2; exit 2; }
H=/usr/local/libexec/k2-root-helper
OLD="$IT/k2-root-helper-old"
NEW="$IT/k2-root-helper-new"
T031="$IT/sew-0.3.1-x86_64-unknown-linux-gnu.tar.gz"
T050="$IT/sew-0.5.0-x86_64-unknown-linux-gnu.tar.gz"
PORT=47123
JWKS_URL="http://127.0.0.1:$PORT/v1/chat/jwks"
SOCK=/run/sew/mgmt.sock
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
for f in "$OLD" "$NEW" "$T031" "$T050" "$IT/install-root-helper.sh"; do [ -f "$f" ] || fail "missing $f"; done
id k2 >/dev/null 2>&1 || useradd -m -s /bin/bash k2
[ -x /usr/lib/postgresql/16/bin/postgres ] || fail "postgresql-16 is not installed"

# ── the fake daemon (as tests/linux/root_helper_in_container.sh) ──────
install -d -o k2 -g k2 -m 0700 /run/k2fd
[ -p /run/k2fd/req ] || { mkfifo /run/k2fd/req && chown k2:k2 /run/k2fd/req; }
cat >/opt/fake-daemon.sh <<'SH'
#!/bin/bash
D=/run/k2fd
while true; do
	read -r id args <"$D/req" || continue
	in=/dev/null
	[ -f "$D/$id.in" ] && in="$D/$id.in"
	# shellcheck disable=SC2086
	set -- $args
	sudo -n /usr/local/libexec/k2-root-helper "$@" <"$in" >"$D/$id.out" 2>"$D/$id.err"
	echo $? >"$D/$id.rc.tmp"
	mv "$D/$id.rc.tmp" "$D/$id.rc"
done
SH
chmod 0755 /opt/fake-daemon.sh
cat >/etc/systemd/system/k2-daemon.service <<'UNIT'
[Unit]
Description=fake K2 daemon (Sew upgrade test)
[Service]
Type=simple
User=k2
ExecStart=/opt/fake-daemon.sh
UNIT

# ── the loopback JWKS (an EC P-256 public key, like K2's keyring) ─────
install -d -m 0755 /opt/jwks/v1/chat
openssl ecparam -name prime256v1 -genkey -noout -out /opt/jwks/key.pem 2>/dev/null
b64u() { base64 -w0 | tr '+/' '-_' | tr -d '='; }
PUBHEX="$(openssl ec -in /opt/jwks/key.pem -pubout -outform DER 2>/dev/null | tail -c 64 | od -An -tx1 | tr -d ' \n')"
X="$(printf '%s' "${PUBHEX:0:64}" | xxd -r -p | b64u)"
Y="$(printf '%s' "${PUBHEX:64:64}" | xxd -r -p | b64u)"
printf '{"keys":[{"kty":"EC","crv":"P-256","x":"%s","y":"%s","kid":"k2it-1","alg":"ES256","use":"sig"}]}\n' "$X" "$Y" >/opt/jwks/v1/chat/jwks
cat >/etc/systemd/system/it-jwks.service <<UNIT
[Unit]
Description=loopback JWKS for the Sew upgrade test
[Service]
Type=simple
ExecStart=/usr/bin/python3 -m http.server $PORT --bind 127.0.0.1 --directory /opt/jwks
UNIT
systemctl daemon-reload
systemctl restart k2-daemon.service it-jwks.service
sleep 1
systemctl is-active --quiet k2-daemon.service || fail "fake daemon did not start"
curl -fsS "$JWKS_URL" | grep -q '"kty":"EC"' || fail "the loopback JWKS does not answer"
ok "fake daemon + loopback JWKS"

N=0
# req [@stdin-file] <helper args…> → RC, OUT (file), ERR
req() {
	N=$((N + 1))
	local id="$N"
	if [ "${1:0:1}" = "@" ]; then
		cp "${1:1}" "/run/k2fd/$id.in"
		chown k2:k2 "/run/k2fd/$id.in"
		shift
	fi
	echo "$id $*" >/run/k2fd/req
	for _ in $(seq 1 6000); do
		[ -f "/run/k2fd/$id.rc" ] && break
		sleep 0.1
	done
	[ -f "/run/k2fd/$id.rc" ] || fail "request $id ($*) timed out"
	RC="$(cat "/run/k2fd/$id.rc")"
	OUT="/run/k2fd/$id.out"
	ERR="$(cat "/run/k2fd/$id.err")"
}
must() { # must <label> <helper args…>: rc 0, and sewExit 0 when it is a Sew run
	local label="$1"
	shift
	req "$@"
	[ "$RC" = 0 ] || fail "$label: rc $RC: $ERR $(head -c 1500 "$OUT")"
	local se
	se="$(jq -r 'if type=="object" then (.sewExit // "none") else "none" end' "$OUT" 2>/dev/null || echo none)"
	[ "$se" = none ] || [ "$se" = 0 ] || fail "$label: sewExit $se: $(head -c 2000 "$OUT")"
}
TOK=""
MB=""
MS=""
mg() { # mg GET|POST <path> [json] → MB (body), MS (HTTP status, 000 = no answer); never in $(…)
	local m="$1" p="$2" body="${3:-}" key out
	key="k2it-$(head -c 9 /dev/urandom | od -An -tx1 | tr -d ' \n')"
	if [ "$m" = GET ]; then
		out="$(runuser -u k2 -- curl -sS --unix-socket "$SOCK" -H "Authorization: Bearer $TOK" -w '\n%{http_code}' "http://sew$p" 2>&1 || true)"
	else
		out="$(runuser -u k2 -- curl -sS --unix-socket "$SOCK" -H "Authorization: Bearer $TOK" -H "Idempotency-Key: $key" \
			-H 'Content-Type: application/json' -H 'Sew-Actor: system:k2it' -X POST -d "$body" -w '\n%{http_code}' "http://sew$p" 2>&1 || true)"
	fi
	MS="${out##*$'\n'}"
	MB="${out%$'\n'*}"
	case "$MS" in [0-9][0-9][0-9]) ;; *) MB="$out"; MS=000 ;; esac
}
wait_version() { # wait_version <want> : mgmt status answers that version
	local v=""
	for _ in $(seq 1 180); do
		req engine sew token
		if [ "$RC" = 0 ]; then
			TOK="$(jq -r .token "$OUT")"
			mg GET /mgmt/v1/status
			v="$(jq -r '.version // empty' <<<"$MB" 2>/dev/null || true)"
			[ "$MS" = 200 ] && [ "${v%%+*}" = "$1" ] && return 0
		fi
		sleep 1
	done
	fail "Sew never answered version $1 over mgmt (last: HTTP $MS '$v' $(head -c 300 <<<"$MB"); units $(systemctl is-active sew.service sew-db.service | tr '\n' ' '); $(ls -l "$SOCK" 2>&1))"
}

install_helper() {
	bash "$IT/install-root-helper.sh" --file "$1" >/tmp/irh.log 2>&1 || { cat /tmp/irh.log; fail "install-root-helper.sh --file $1"; }
	local v
	v="$(runuser -u k2 -- sudo -n "$H" helper version)"
	echo "    helper: $v"
}

echo "== 1. the OLD helper installs Sew 0.3.1 in K2 mode =="
install_helper "$OLD"
must "status (old)" engine sew status
[ "$(jq -r .pin.version "$OUT")" = 0.3.1 ] || fail "the old helper pins 0.3.1: $(cat "$OUT")"
must "stage 0.3.1" "@$T031" engine sew stage
must "install-bin 0.3.1" engine sew install-bin
KEY="$(head -c 32 /dev/urandom | b64u)"
printf '{"jwksUrl":"%s","masterKey":"%s"}' "$JWKS_URL" "$KEY" >/tmp/install.json
must "install (k2 mode)" @/tmp/install.json engine sew install
must "enable" engine sew systemctl enable
must "start-db" engine sew systemctl start-db
must "migrate" engine sew systemctl migrate
must "start" engine sew systemctl start
if jq -e '.allowUid.restartNeeded == true' "/run/k2fd/$((N - 4)).out" >/dev/null 2>&1; then must "restart (allow_uids)" engine sew systemctl restart; fi
wait_version 0.3.1
mg GET /mgmt/v1/status; ST="$MB"
[ "$(jq -r .provider <<<"$ST")" = k2 ] || fail "provider k2: $ST"
INSTALL_ID="$(jq -r .install_id <<<"$ST")"
ok "Sew 0.3.1 up in K2 mode (install $INSTALL_ID; features: $(jq -c '.features // []' <<<"$ST"))"
P1="k2:principal:$(cat /proc/sys/kernel/random/uuid)"
mg POST /mgmt/v1/principals/provision "$(printf '{"external_id":"%s","kind":"person","guest":false,"profile":{"display_name":"Ann Seed","username":"annseed","title":"CTO","pronouns":"she/her"}}' "$P1")"; R="$MB"
[ "$MS" = 200 ] || fail "seed provision: $MS $R"
M1="$(jq -r .member_id <<<"$R")"
ok "seeded member $M1 ($P1)"
# The box owner, as K2's enable does (chat::owner_ops: provision + owner/set).
mg POST /mgmt/v1/principals/provision '{"external_id":"k2:owner","kind":"person","guest":false,"profile":{"display_name":"Owner","username":"owner"}}'
[ "$MS" = 200 ] || fail "owner provision: $MS $MB"
mg POST /mgmt/v1/owner/set '{"external_id":"k2:owner","force":false}'
[ "$MS" = 200 ] || fail "owner/set: $MS $MB"
ok "k2:owner provisioned and set as the install owner"

echo "== 2. the NEW helper upgrades 0.3.1 → 0.5.0 =="
install_helper "$NEW"
must "status (new)" engine sew status
[ "$(jq -r .pin.version "$OUT")" = 0.5.0 ] || fail "the new helper pins 0.5.0: $(cat "$OUT")"
[ "$(jq -r '.binary.version' "$OUT")" = 0.3.1 ] && [ "$(jq -r '.binary.known' "$OUT")" = true ] || fail "the new helper knows the installed 0.3.1: $(jq -c .binary "$OUT")"
must "stage 0.5.0" "@$T050" engine sew stage
must "upgrade" engine sew upgrade
echo "    upgrade: $(jq -c '{sewExit, from, to, source, sew}' "$OUT" | head -c 600)"
[ "$(jq -r .from "$OUT")" = 0.3.1 ] && [ "$(jq -r .to "$OUT")" = 0.5.0 ] || fail "upgrade from/to: $(cat "$OUT")"
# What K2 records as the rollback point (supervisor::ROLLBACK_POINT_RECORD).
PRE_BUNDLE="$(jq -r '.sew.pre_bundle // empty' "$OUT")"
[ -n "$PRE_BUNDLE" ] || fail "Sew's upgrade answer names its pre-upgrade bundle (pre_bundle): $(cat "$OUT")"
wait_version 0.5.0
mg GET /mgmt/v1/status; ST="$MB"
[ "$(jq -r .install_id <<<"$ST")" = "$INSTALL_ID" ] || fail "same install after the upgrade: $ST"
FEATS="$(jq -r '.features | sort | join(",")' <<<"$ST")"
WANT_FEATS="app_channel_create,app_origin,app_quotas,app_scoped_sessions,home_client,k2_managed_avatar,member_external_id,person_notifications,principal_relink,private_spaces,space_unbind,voice_video_cap"
[ "$FEATS" = "$WANT_FEATS" ] || fail "0.5.0's 12 features: got $FEATS"
mg GET "/mgmt/v1/principals?external_id=$P1&limit=1"; LIST="$MB"
[ "$(jq -r '.items[0].member_id' <<<"$LIST")" = "$M1" ] || fail "the seeded member survives: $LIST"
[ "$(jq -r '.items[0].title' <<<"$LIST")" = CTO ] || fail "its title survives: $LIST"
ok "upgraded to 0.5.0 (same install, 12 features, the member and its title kept)"

echo "== 3. Sew 0.5.0 takes K2's contract =="
mg GET /mgmt/v1/policies; POL="$MB"
[ "$MS" = 200 ] || fail "GET policies: $MS $POL"
echo "    managed_fields before: $(jq -c '.policies["identity.managed_fields"]' <<<"$POL")"
mg POST /mgmt/v1/policies '{"changes":{"identity.managed_fields":["display_name","title","pronouns","avatar"]}}'; POL="$MB"
[ "$MS" = 200 ] || fail "POST policies (K2's body): $MS $POL"
[ "$(jq -r '.policies["identity.managed_fields"] | sort | join(",")' <<<"$POL")" = "avatar,display_name,pronouns,title" ] || fail "K2's set back: $POL"
mg POST /mgmt/v1/policies '{"changes":{"identity.managed_fields":["display_name","title","pronouns","avatar"]}}'
[ "$MS" = 200 ] || fail "the same set again is a no-op 200: $MS"
mg POST /mgmt/v1/policies '{"changes":{"identity.managed_fields":["display_name","display_name"]}}'; BAD="$MB"
[ "$MS" = 400 ] && [ "$(jq -r .error.code <<<"$BAD")" = validation_failed ] || fail "a duplicate is 400 validation_failed: $MS $BAD"
ok "managed-fields policy: K2's body set, idempotent, a duplicate refused"
mg POST /mgmt/v1/principals/provision "$(printf '{"external_id":"%s","kind":"person","guest":true,"profile":{"display_name":"Ann Seed","username":"annseed","title":"CTO","pronouns":"she/her"}}' "$P1")"; R="$MB"
[ "$MS" = 200 ] || fail "re-provision with title/pronouns: $MS $R"
mg GET "/mgmt/v1/principals?external_id=$P1&limit=1"; LIST="$MB"
[ "$(jq -r '.items[0].title + "|" + .items[0].pronouns' <<<"$LIST")" = "CTO|she/her" ] || fail "option (a): a provision that carries them keeps them: $LIST"
ok "a provision carrying title/pronouns keeps them (option a)"
mg POST /mgmt/v1/spaces/create '{"slug":"k2it-jobs","name":"K2 IT Jobs","owner":"k2:owner","app_id":"k2it-app"}'; SP="$MB"
case "$MS" in 200|201) ;; *) fail "spaces/create with app_id: $MS $SP" ;; esac
SPACE="$(jq -r '.space_id // .id' <<<"$SP")"
mg POST /mgmt/v1/spaces/bind "$(printf '{"space_id":"%s","app_id":"k2it-app"}' "$SPACE")"
[ "$MS" = 200 ] || fail "spaces/bind (the same App) is a no-op 200: $MS"
prov() { # prov <external id> <username>: a plain member
	mg POST /mgmt/v1/principals/provision "$(printf '{"external_id":"%s","kind":"person","guest":false,"profile":{"display_name":"%s","username":"%s"}}' "$1" "$2" "$2")"
	[ "$MS" = 200 ] || fail "provision $1: $MS $MB"
}
P2="k2:principal:$(cat /proc/sys/kernel/random/uuid)"
P3="k2:principal:$(cat /proc/sys/kernel/random/uuid)"
prov "$P2" bo2it
prov "$P3" cy3it
add() { mg POST /mgmt/v1/spaces/members/add "$(printf '{"space_id":"%s","app_id":"k2it-app","external_ids":["%s"]}' "$SPACE" "$1")"; }
add "$P2"
[ "$MS" = 200 ] && [ "$(jq -r '.added[0].external_id' <<<"$MB")" = "$P2" ] || fail "members/add before unbind: $MS $MB"
mg POST /mgmt/v1/spaces/unbind "$(printf '{"space_id":"%s","app_id":"k2it-app"}' "$SPACE")"; UN="$MB"
[ "$MS" = 200 ] || fail "spaces/unbind: $MS $UN"
[ "$(jq -r .orphaned_from <<<"$UN")" = k2it-app ] && [ "$(jq -r .changed <<<"$UN")" = true ] || fail "unbind answer: $UN"
mg POST /mgmt/v1/spaces/unbind "$(printf '{"space_id":"%s","app_id":"k2it-app"}' "$SPACE")"; UN2="$MB"
[ "$MS" = 200 ] && [ "$(jq -r .changed <<<"$UN2")" = false ] || fail "unbind again is changed:false: $MS $UN2"
add "$P3"
[ "$MS" = 422 ] && [ "$(jq -r .error.details.reason <<<"$MB")" = space_orphaned ] || fail "an add to the orphan is 422 space_orphaned: $MS $MB"
mg POST /mgmt/v1/channels/create "$(printf '{"space_id":"%s","app_id":"k2it-app","slug":"c1","name":"C1","private":false}' "$SPACE")"
[ "$MS" = 422 ] && [ "$(jq -r .error.details.reason <<<"$MB")" = space_orphaned ] || fail "a channel create in the orphan is 422 space_orphaned: $MS $MB"
# Removes still work on an orphan (Sew 0.5.0): K2's --remove-app-members
# never strands, whatever the order.
mg POST /mgmt/v1/spaces/members/remove "$(printf '{"space_id":"%s","app_id":"k2it-app","external_id":"%s"}' "$SPACE" "$P2")"
[ "$MS" = 200 ] && [ "$(jq -r .removed <<<"$MB")" = true ] || fail "a remove from the orphan works: $MS $MB"
mg POST /mgmt/v1/spaces/bind "$(printf '{"space_id":"%s","app_id":"k2it-other"}' "$SPACE")"; E="$MB"
[ "$MS" = 409 ] && [ "$(jq -r .error.details.reason <<<"$E")" = space_bound_elsewhere ] || fail "bind elsewhere is 409 space_bound_elsewhere: $MS $E"
mg POST /mgmt/v1/spaces/bind "$(printf '{"space_id":"%s","app_id":"k2it-app"}' "$SPACE")"
[ "$MS" = 200 ] || fail "the same App binds its orphan again: $MS $MB"
add "$P3"
[ "$MS" = 200 ] && [ "$(jq -r '.added[0].external_id' <<<"$MB")" = "$P3" ] || fail "writable again after the same App binds: $MS $MB"
ok "unbind: orphaned_from, idempotent; adds and channel creates 422 space_orphaned, removes still work; another App 409 space_bound_elsewhere; the same App rebinds"
mg POST /mgmt/v1/home-client '{"app_id":"k2it-app"}'
[ "$MS" = 200 ] || fail "home-client: $MS"
mg POST /mgmt/v1/apps/k2it-app/settings '{"hidden_from_home":true,"display_name":"K2 IT"}'
[ "$MS" = 200 ] || fail "App settings: $MS"
ok "home client and App settings taken"

echo "== 4. engine sew install reconfigure over 0.5.0 =="
printf '{"jwksUrl":"%s"}' "$JWKS_URL" >/tmp/reconf.json
must "install reconfigure" @/tmp/reconf.json engine sew install reconfigure
echo "    reconfigure: $(jq -c '{sewExit, sew}' "$OUT" | head -c 400)"
must "restart after reconfigure" engine sew systemctl restart
wait_version 0.5.0
mg GET "/mgmt/v1/principals?external_id=$P1&limit=1"; LIST="$MB"
[ "$(jq -r '.items[0].member_id' <<<"$LIST")" = "$M1" ] || fail "member after reconfigure: $LIST"
must "doctor" engine sew doctor
echo "    doctor: $(jq -c '.sew.summary // .summary // {sewExit}' "$OUT" | head -c 300)"
ok "reconfigured; healthy; data kept"

echo "== 5. the plain rollback refuses across the schema change =="
req engine sew rollback
echo "    rollback: rc $RC $(jq -c '{sewExit, sew}' "$OUT" 2>/dev/null | head -c 600) $ERR"
[ "$RC" = 0 ] && [ "$(jq -r .sewExit "$OUT")" = 3 ] || fail "the plain rollback is refused (exit 3): rc $RC $(head -c 800 "$OUT")"
jq -r '.sew.error.message // ""' "$OUT" | grep -q "accept-data-loss-since" || fail "Sew's refusal names --accept-data-loss-since: $(cat "$OUT")"
wait_version 0.5.0
mg GET "/mgmt/v1/principals?external_id=$P1&limit=1"
[ "$(jq -r '.items[0].member_id' <<<"$MB")" = "$M1" ] || fail "member after a refused rollback: $MB"
ok "a plain rollback is refused (exit 3) and 0.5.0 stays healthy with its data"

echo "== 6. a message written on 0.5.0 =="
AID="$(cat /proc/sys/kernel/random/uuid)"
A="k2:workspace:$AID"
mg POST /mgmt/v1/principals/provision "$(printf '{"external_id":"%s","kind":"agent","agent_kind":"coworker","guest":false,"profile":{"display_name":"K2 IT bot","username":"k2itbot"}}' "$A")"
[ "$MS" = 200 ] || fail "provision the writing agent: $MS $MB"
mg POST /mgmt/v1/spaces/create "$(printf '{"slug":"k2it-msgs","name":"K2 IT messages","owner":"k2:owner","managers":["%s"],"app_id":"k2it-msg"}' "$A")"
case "$MS" in 200|201) ;; *) fail "a Space for the message: $MS $MB" ;; esac
MSPACE="$(jq -r '.space_id' <<<"$MB")"
mg POST /mgmt/v1/channels/create "$(printf '{"space_id":"%s","app_id":"k2it-msg","slug":"general","name":"General","private":false,"members":["%s"]}' "$MSPACE" "$A")"
[ "$MS" = 200 ] || fail "a channel for the message: $MS $MB"
CH="$(jq -r .channel_id <<<"$MB")"
VIA="k2; agent=$AID; k2_session=k2it-1"
OUTM="$(runuser -u k2 -- curl -sS --unix-socket "$SOCK" -H "Authorization: Bearer $TOK" -H "Sew-Act-As: $A" -H "Sew-Via: $VIA" \
	-H "Idempotency-Key: k2it-msg-1" -H 'Content-Type: application/json' -X POST -d '{"body":"written on 0.5.0, lost by the rollback"}' \
	-w '\n%{http_code}' "http://sew/api/v1/channels/$CH/messages" 2>&1 || true)"
[ "${OUTM##*$'\n'}" = 200 ] || [ "${OUTM##*$'\n'}" = 201 ] || fail "post a message as the agent: $OUTM"
MID="$(jq -r .id <<<"${OUTM%$'\n'*}")"
ok "message $MID written on 0.5.0 (channel $CH)"

echo "== 7. the emergency rollback, in K2's order =="
must "helper version (rollback verb)" helper version
[ "$(jq -r .namespaces.sewrbloss "$OUT")" = true ] || fail "the new helper serves the emergency rollback: $(cat "$OUT")"
# 1. Back up 0.5.0 first (the mgmt backup job, as the daemon does).
mg POST /mgmt/v1/backup '{}'
[ "$MS" = 200 ] || [ "$MS" = 202 ] || fail "start the backup: $MS $MB"
JOB="$(jq -r .job_id <<<"$MB")"
KEPT=""
for _ in $(seq 1 600); do
	mg GET "/mgmt/v1/jobs/$JOB"
	case "$(jq -r .state <<<"$MB")" in
		succeeded) KEPT="$(jq -r .result.id <<<"$MB")"; break ;;
		failed|dead) fail "the backup job failed: $MB" ;;
	esac
	sleep 1
done
[ -n "$KEPT" ] || fail "the backup job never finished: $MB"
ok "0.5.0 backed up first: $KEPT"
# 2.-3. Back to the pre-upgrade bundle, accepting the loss since it.
req engine sew rollback accept-data-loss "$PRE_BUNDLE"
echo "    emergency rollback: rc $RC $(jq -c '{sewExit, bundle, upgradeState, sew}' "$OUT" 2>/dev/null | head -c 1200) $ERR"
[ "$RC" = 0 ] && [ "$(jq -r .sewExit "$OUT")" = 0 ] || fail "the emergency rollback: rc $RC $(head -c 1500 "$OUT")"
[ "$(jq -r .bundle "$OUT")" = "$PRE_BUNDLE" ] || fail "it rolled back to the recorded bundle: $(cat "$OUT")"
# 4. K2's checks: healthy on 0.3.1, doctor 0 fail.
wait_version 0.3.1
must "doctor after the rollback" engine sew doctor
FAILS="$(jq -r '[.sew.checks[]? | select(.status == "fail") | .id] | join(",")' "$OUT")"
[ -z "$FAILS" ] || fail "sew doctor reports failures after the rollback: $FAILS"
ok "Sew is back on a healthy 0.3.1; doctor 0 fail"
mg GET "/mgmt/v1/principals?external_id=$P1&limit=1"
[ "$(jq -r '.items[0].member_id' <<<"$MB")" = "$M1" ] || fail "the pre-upgrade member is back: $MB"
[ "$(jq -r '.items[0].install_role' <<<"$MB")" != guest ] || fail "its pre-upgrade role (not the guest flip made on 0.5.0): $MB"
mg GET "/mgmt/v1/principals?external_id=$A&limit=1"
[ "$(jq -r '.items | length' <<<"$MB")" = 0 ] || fail "the agent written on 0.5.0 is gone from the live chat: $MB"
ok "pre-upgrade data back; what was written on 0.5.0 (the agent, its Space and message) is gone from the live chat"
[ -e "/var/lib/sew/backups/sew-backup-$KEPT" ] || fail "the 0.5.0 backup $KEPT is kept on disk: $(ls /var/lib/sew/backups)"
mg GET /mgmt/v1/backups
echo "    0.3.1 lists the kept bundle: $(jq -c --arg k "$KEPT" '[.items[] | select(.id == $k) | {id, sew_version, kind}]' <<<"$MB")"
ok "the 0.5.0 backup $KEPT is kept (restorable onto 0.5.x later)"
echo "RESULT rollback=emergency_rolled_back kept=$KEPT bundle=$PRE_BUNDLE"
echo "sew upgrade (real): $pass checks passed"
