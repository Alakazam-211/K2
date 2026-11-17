#!/usr/bin/env bash
# Hermetic test for step 9c (Sew chat + voice, Home chat, voice rooms) of
# scripts/provision-k2-server.sh. The provisioner has no dry-run mode and
# must never run for real here, so this test:
#   1. checks the script's shape statically (9c sits after daemon readiness,
#      the root helper check and the owner user, before the callback; the
#      CLI runs as the daemon user with a clean env; K2_SKIP_SEW is
#      documented; the summary prints the line), and
#   2. cuts the 9c block out of the script and runs it alone with a fake
#      `k2` (k2_as_daemon_user stub honouring K2_CLI_TIMEOUT) and PATH shims
#      for curl/sleep/timeout.
#      No root, no network, no daemon.
# Run with: bash tests/cli/provision_sew.sh
# Literal $… in single quotes and A && B || fail are intended here.
# shellcheck disable=SC2015,SC2016,SC2028
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# Everything the block calls is faked (k2, timeout, curl, sleep) except
# python3, which the provisioner itself needs (step 1 installs it).
command -v python3 >/dev/null 2>&1 || { echo "SKIP: provision_sew.sh needs python3"; exit 0; }
PROVISION="$ROOT/scripts/provision-k2-server.sh"
[ -f "$PROVISION" ] || { echo "FAIL: missing $PROVISION" >&2; exit 1; }

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

echo "== static shape =="
bash -n "$PROVISION" || fail "bash -n"
ok "bash -n"
line_of() { grep -n -- "$1" "$PROVISION" | head -n1 | cut -d: -f1; }
L_HELPER="$(line_of '# ── 7a. root helper')"
L_READY="$(line_of '# ── 8. wait for daemon readiness')"
L_OWNER="$(line_of '# ── 9. first owner user')"
L_OPS="$(line_of '# ── 9b. website-management')"
L_SEW="$(line_of '# ── 9c. Sew')"
L_CB="$(line_of '# ── 10. control-plane callback')"
for v in L_HELPER L_READY L_OWNER L_OPS L_SEW L_CB; do [ -n "${!v}" ] || fail "marker $v not found"; done
[ "$L_HELPER" -lt "$L_SEW" ] && [ "$L_READY" -lt "$L_SEW" ] && [ "$L_OWNER" -lt "$L_SEW" ] && [ "$L_OPS" -lt "$L_SEW" ] \
	|| fail "9c must come after the root helper, daemon readiness and the owner/ops users"
[ "$L_SEW" -lt "$L_CB" ] || fail "9c must run before the callback flips the server online"
ok "9c runs after 7a/8/9/9b and before the callback (10)"
BLOCK="$(sed -n '/^# ── 9c\. Sew/,/^# ── 10\./p' "$PROVISION" | sed '$d')"
[ -n "$BLOCK" ] || fail "9c block not found"
grep -q 'runuser -u "$K2_RUN_USER" -- env -i HOME="$K2_HOME" USER="$K2_RUN_USER"' "$PROVISION" \
	|| fail "k2_as_daemon_user must drop to the daemon user with a clean env"
if <<<"$(<<<"$BLOCK" grep -Ev '^[[:space:]]*#')" grep -Eq '(^|[;&|(]|then|else|do)[[:space:]]*(sudo|runuser|/usr/local/bin/k2|k2)[[:space:]]'; then
	fail "9c must run the CLI only through k2_as_daemon_user"
fi
ok "the CLI runs only through k2_as_daemon_user (daemon user, env -i)"
grep -q '^#   K2_SKIP_SEW ' "$PROVISION" || fail "K2_SKIP_SEW must be documented in the header"
grep -q 'echo "  sew:       ${SEW_LINE}"' "$PROVISION" || fail "the summary must print the sew line"
ok "K2_SKIP_SEW documented; summary prints the sew line"
grep -q '_bound=(timeout --kill-after=30s "$K2_CLI_TIMEOUT")' "$PROVISION" \
	&& grep -q '${_bound\[@\]+"${_bound\[@\]}"} runuser' "$PROVISION" \
	|| fail "k2_as_daemon_user must bound a call with K2_CLI_TIMEOUT"
if <<<"$(<<<"$BLOCK" grep -Ev '^[[:space:]]*#')" grep -Eiq 'latest|--tarball|--version'; then
	fail "9c must not override K2's Sew pin (no latest / --tarball / --version)"
fi
ok "long steps are time-bounded; nothing overrides K2's Sew pin"

WORK="$(mktemp -d -t k2-provision-sew-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
mkdir -p "$SHIMS"
cat >"$SHIMS/curl" <<'SH'
#!/bin/sh
# Only /boot-status is fetched in 9c.
echo x >>"$FAKE_STATE/boot-polls.log"
[ -n "${FAKE_BOOT+x}" ] || FAKE_BOOT='{"phase":"ready","features":["home-chat-v1","voice-rooms-v1"]}'
printf '%s\n' "$FAKE_BOOT"
SH
cat >"$SHIMS/sleep" <<'SH'
#!/bin/sh
exit 0
SH
cat >"$SHIMS/timeout" <<'SH'
#!/bin/sh
# timeout <limit> k2 <args>: record the limit; FAKE_HANG=<args> times out.
# Shift "k2" off explicitly: ${*#k2 } is shell-dependent (dash/busybox strip
# the joined string, bash strips each word, so "k2" survived where /bin/sh
# is bash). Never the host's timeout(1): the test owns this one.
limit="$1"; shift
[ "${1:-}" = k2 ] || { echo "timeout shim: expected k2, got: $*" >&2; exit 98; }
shift
echo "$limit $*" >>"$FAKE_STATE/timeouts.log"
[ "${FAKE_HANG:-}" = "$*" ] && exit 124
exec k2 "$@"
SH
cat >"$SHIMS/k2" <<'SH'
#!/usr/bin/env bash
# Fake CLI. State in $FAKE_STATE (installed, identity, voice, backup-age);
# every call appended to calls.log.
S="$FAKE_STATE"
echo "$*" >>"$S/calls.log"
rd() { cat "$S/$1" 2>/dev/null || true; }
case "$*" in
	"hostchat status --json")
		echo "Sew status (fake)"
		[ -z "${FAKE_STATUS_RAW+x}" ] || { printf '%b' "$FAKE_STATUS_RAW"; exit "${FAKE_STATUS_RC:-0}"; }
		# Shapes as K2 0.46.14 prints them: not-installed has null
		# installedVersion/identityProvider and the pin.
		python3 -I -c 'import json,sys
inst,idp,voice,age,pin,state=sys.argv[1:7]
d={"state":"not-installed","ok":True,"installedVersion":None,"pinnedVersion":pin,
   "identityProvider":None,"voice":{"state":voice or "stopped"},"backup":{}}
if inst: d.update(state=state or "running", installedVersion=inst, identityProvider=idp)
if age: d["backup"]={"ageMs":int(age),"lastOkAt":"2026-11-17T00:00:00Z","lastId":"b0"}
print(json.dumps(d))' "$(rd installed)" "$(rd identity)" "$(rd voice)" "$(rd backup-age)" "${FAKE_PINNED-0.10.3}" "$(rd state)"
		exit "${FAKE_STATUS_RC:-0}" ;;
	"hostchat preflight") printf '%b' "${FAKE_PREFLIGHT-ready\nall checks pass\n}" ;;
	"hostchat enable --identity k2")
		[ "${FAIL_ENABLE:-0}" = 1 ] && { echo "enable boom" >&2; exit 3; }
		echo "${FAKE_INSTALL_VERSION-0.10.3}" >"$S/installed"; echo k2 >"$S/identity"; echo "linked" ;;
	"hostchat media install")
		[ "${FAIL_MEDIA:-0}" = 1 ] && { echo "media boom" >&2; exit 3; }
		[ "${MEDIA_NOOP:-0}" = 1 ] || echo running >"$S/voice"; echo "installed" ;;
	"settings set homeChat true"|"settings set voiceRoomsPreview true") echo "ok" ;;
	"hostchat backup")
		[ "${FAIL_BACKUP:-0}" = 1 ] && { echo "backup boom" >&2; exit 3; }
		echo 5 >"$S/backup-age"; echo '{"id":"b1"}' ;;
	"hostchat doctor --json")
		[ -n "${FAKE_DOCTOR+x}" ] || FAKE_DOCTOR='checking...\n{"verdict":{"failed":[],"degraded":["drill.recent","keys.mirror","keys.swap","search.encrypted_disk","media.turn","media.reachability"]}}\n'
		printf '%b' "$FAKE_DOCTOR"
		[ "${DOCTOR_RC:-0}" = 0 ] || exit "$DOCTOR_RC" ;;
	# 9c.8: the recorder, install only. FAKE_REC_PREFLIGHT is the
	# preflight's text; FAIL_REC_INSTALL=1 fails the install.
	"hostchat recording preflight")
		[ -n "${FAKE_REC_PREFLIGHT+x}" ] || FAKE_REC_PREFLIGHT='ready\n  ok    ram            16.0 GiB of RAM\n'
		printf '%b' "$FAKE_REC_PREFLIGHT" ;;
	"hostchat recording install")
		[ "${FAIL_REC_INSTALL:-0}" = 1 ] && { echo '{"error": "model_does_not_fit"}' >&2; exit 3; }
		echo 1 >"$S/recorder"; echo "recorder:        installed"; echo "recording:       off" ;;
	"hostchat recording status --json")
		if [ -f "$S/recorder" ]; then echo '{"ok": true, "installed": true, "enabled": false}'
		else echo '{"ok": true, "installed": false, "enabled": false}'; fi ;;
	*) echo "fake k2: unexpected $*" >&2; exit 99 ;;
esac
SH
chmod +x "$SHIMS"/*

STEP="$WORK/step.sh"
{
	echo 'set -euo pipefail'
	echo 'log() { printf "[provision] %s\n" "$*"; }'
	echo 'die() { printf "[provision] ERROR: %s\n" "$*" >&2; exit 1; }'
	echo 'k2_as_daemon_user() { local _bound=(); [ -z "${K2_CLI_TIMEOUT:-}" ] || _bound=(timeout "$K2_CLI_TIMEOUT"); ${_bound[@]+"${_bound[@]}"} k2 "$@"; }'
	printf '%s\n' "$BLOCK"
	echo 'echo "SUMMARY=${SEW_LINE}"'
} >"$STEP"

run_step() { # <box>; PRE_INSTALLED=<ver> PRE_IDENTITY PRE_VOICE=1 PRE_BACKUP_AGE=<ms>
	export FAKE_STATE="$WORK/$1"
	rm -rf "$FAKE_STATE"; mkdir -p "$FAKE_STATE"; : >"$FAKE_STATE/calls.log"
	if [ -n "${PRE_INSTALLED:-}" ]; then
		echo "$PRE_INSTALLED" >"$FAKE_STATE/installed"; echo "${PRE_IDENTITY:-k2}" >"$FAKE_STATE/identity"
		[ -z "${PRE_STATE:-}" ] || echo "$PRE_STATE" >"$FAKE_STATE/state"
	fi
	[ "${PRE_VOICE:-0}" = 1 ] && echo running >"$FAKE_STATE/voice"
	[ -n "${PRE_BACKUP_AGE:-}" ] && echo "$PRE_BACKUP_AGE" >"$FAKE_STATE/backup-age"
	set +e
	out="$(PATH="$SHIMS:$PATH" K2_RUN_USER=k2 BASE="http://127.0.0.1:1" \
		ROOT_HELPER_MODE="${ROOT_HELPER_MODE-root}" MAIL_HELPER_LINE="x" bash "$STEP" 2>&1)"
	rc=$?
	set -e
	calls="$(cat "$FAKE_STATE/calls.log")"
}
mutating() { <<<"$calls" grep -v '^hostchat status --json$' || true; }

echo "== fresh box: enable → backup → media → switches → doctor, features checked =="
run_step box1
[ "$rc" -eq 0 ] || fail "success exit $rc: $out"
printf '%s\n' "$out" | sed 's/^/    | /'
want="hostchat preflight
hostchat enable --identity k2
hostchat backup
hostchat media install
settings set homeChat true
settings set voiceRoomsPreview true
hostchat doctor --json
hostchat recording preflight
hostchat recording install
hostchat recording status --json"
[ "$(mutating)" = "$want" ] || fail "call order: $(mutating)"
<<<"$out" grep -q 'Sew \[2/7\]: hostchat preflight' || fail "state not-installed must run preflight + enable: $out"
ok "status state not-installed (null installedVersion/identityProvider) → preflight + enable run"
ok "preflight → enable → backup (before media) → media install → homeChat → voiceRoomsPreview → doctor"
[ "$(grep -v ' hostchat status --json$' "$FAKE_STATE/timeouts.log")" = "20m hostchat enable --identity k2
30m hostchat backup
15m hostchat media install
2m hostchat recording preflight
15m hostchat recording install
2m hostchat recording status --json" ] || fail "time limits: $(cat "$FAKE_STATE/timeouts.log")"
ok "enable 20m, backup 30m, media install 15m, recorder install 15m"
<<<"$out" grep -q 'Sew: recorder installed, recording off' || fail "recorder line: $out"
if <<<"$calls" grep -q 'recording enable\|recording policy'; then fail "provisioning must never turn recording on: $calls"; fi
ok "9c.8: the recorder is installed after the doctor and recording stays off (no enable, no policy)"
[ "$(<<<"$calls" grep -c '^hostchat status --json$')" -eq 4 ] || fail "status must be re-read after enable, backup and media: $calls"
grep -q '^2m hostchat status --json$' "$FAKE_STATE/timeouts.log" || fail "status reads must be time-bounded"
ok "status read at start and re-read after enable, backup and media install (2m bound)"
<<<"$out" grep -q 'installedVersion 0.10.3 pinnedVersion 0.10.3' || fail "version log: $out"
<<<"$out" grep -q 'SUMMARY=installed (Sew 0.10.3, identity k2, voice running, home-chat-v1, voice-rooms-v1)' || fail "summary: $out"
ok "logs installed/pinned version; summary names Sew version, identity, voice and features"
ok "degraded-only doctor verdict (fresh box) passes"

echo "== re-run on a linked box: preflight, enable, backup (<1 h) and media skipped =="
PRE_INSTALLED=0.10.3 PRE_VOICE=1 PRE_BACKUP_AGE=1200000 run_step box2
[ "$rc" -eq 0 ] || fail "re-run exit $rc: $out"
[ "$(mutating)" = "settings set homeChat true
settings set voiceRoomsPreview true
hostchat doctor --json
hostchat recording preflight
hostchat recording install
hostchat recording status --json" ] || fail "re-run calls: $(mutating)"
<<<"$out" grep -q 'last backup is 20 min old — skipped' || fail "backup skip line: $out"
ok "idempotent re-run; a backup under 1 h old is not repeated"
PRE_INSTALLED=0.10.3 PRE_VOICE=1 PRE_BACKUP_AGE=3600000 run_step box2b
[ "$rc" -eq 0 ] && <<<"$calls" grep -qx 'hostchat backup' || fail "a 1 h old backup must be redone: $calls"
ok "a backup 1 h or older is taken again"

echo "== an existing non-k2 Sew is never converted =="
PRE_INSTALLED=0.10.3 PRE_IDENTITY=standalone run_step std
[ "$rc" -ne 0 ] || fail "standalone must stop: $out"
<<<"$out" grep -q 'Sew is installed with identity standalone; changing it is a migration, not a provisioning step' || fail "standalone message: $out"
[ -z "$(mutating)" ] || fail "nothing may run on a standalone Sew: $(mutating)"
ok "standalone Sew: stops before preflight/enable, with the migration message"
PRE_INSTALLED=0.10.3 PRE_IDENTITY=standalone PRE_STATE=degraded run_step std2
[ "$rc" -ne 0 ] && <<<"$out" grep -q 'Sew is installed with identity standalone; changing it is a migration' || fail "degraded standalone: $out"
[ -z "$(mutating)" ] || fail "nothing may run on a degraded standalone Sew: $(mutating)"
ok "state degraded + standalone (sewlab): refusal"
FAKE_STATUS_RAW='{"state":"degraded","ok":false,"installedVersion":null,"identityProvider":null}\n' run_step std3
[ "$rc" -ne 0 ] && <<<"$out" grep -q 'changing it is a migration' || fail "installed-state with no identity must refuse: $out"
[ -z "$(mutating)" ] || fail "nothing may run: $(mutating)"
ok "any state other than not-installed without identity k2: refusal"

echo "== unreadable status: never guess, stop before enable =="
for raw in 'Error: daemon busy\n' '' 'Sew status\n[1,2]\n'; do
	FAKE_STATUS_RAW="$raw" run_step unr
	[ "$rc" -ne 0 ] || fail "unreadable status ($raw) must stop: $out"
	<<<"$out" grep -q 'sew step failed: hostchat status --json unreadable; refusing to guess whether Sew is installed' || fail "message ($raw): $out"
	[ -z "$(mutating)" ] || fail "nothing may run after an unreadable status ($raw): $(mutating)"
done
FAKE_STATUS_RC=1 run_step unr2
[ "$rc" -ne 0 ] && <<<"$out" grep -q 'hostchat status --json unreadable' || fail "a failing status must stop: $out"
[ -z "$(mutating)" ] || fail "nothing may run after a failed status: $(mutating)"
FAKE_HANG="hostchat status --json" run_step unr3
[ "$rc" -ne 0 ] && <<<"$out" grep -q 'hostchat status --json unreadable' || fail "a timed-out status must stop: $out"
ok "non-JSON / empty / non-object / failed / timed-out status stops before preflight and enable"

echo "== not Pro: voice-rooms-v1 missing is a NOTE, not a failure =="
FAKE_BOOT='{"features":["home-chat-v1"]}' run_step box3
[ "$rc" -eq 0 ] || fail "no voice rooms must pass: $out"
<<<"$out" grep -q 'NOTE: Sew: /boot-status does not list voice-rooms-v1' || fail "note: $out"
ok "voice-rooms-v1 missing → NOTE"

echo "== failures stop provisioning with 'sew step failed: <step>' =="
expect_fail() { # <what> <message fragment>
	[ "$rc" -ne 0 ] || fail "$1 must fail: $out"
	<<<"$out" grep -q "sew step failed: $2" || fail "$1 message: $out"
	if <<<"$out" grep -q 'SUMMARY='; then fail "$1 must not continue"; fi
}
FAKE_PREFLIGHT='not ready: voice ports\n' run_step f1
expect_fail "preflight not ready" "hostchat preflight"
<<<"$out" grep -q 'not ready: voice ports' || fail "preflight output must be shown: $out"
if <<<"$calls" grep -q enable; then fail "nothing runs after a failed preflight"; fi
FAIL_ENABLE=1 run_step f2
expect_fail "enable" "hostchat enable --identity k2 (exit 3)"
<<<"$out" grep -q 'enable boom' || fail "enable output must be shown: $out"
FAKE_INSTALL_VERSION=0.10.2 run_step f2v
expect_fail "version mismatch" "Sew version check (installedVersion '0.10.2' is not K2's pinnedVersion '0.10.3')"
FAKE_PINNED="" run_step f2p
expect_fail "missing pin" "Sew version check"
FAIL_BACKUP=1 run_step f3
expect_fail "backup" "hostchat backup (exit 3)"
if <<<"$calls" grep -q 'media install'; then fail "media must not run after a failed backup"; fi
FAIL_MEDIA=1 run_step f4
expect_fail "media install" "hostchat media install (exit 3)"
MEDIA_NOOP=1 run_step f5
expect_fail "voice not running" "hostchat media install (status voice.state is not running)"
FAKE_DOCTOR='{"verdict":{"failed":["voice-turn"],"degraded":[]}}\n' DOCTOR_RC=1 run_step f6
expect_fail "doctor verdict" 'hostchat doctor (verdict.failed: \["voice-turn"\])'
FAKE_DOCTOR='garbage\n' run_step f7
expect_fail "unreadable doctor" "hostchat doctor --json (unreadable verdict)"
FAKE_BOOT='{"features":["voice-rooms-v1"]}' run_step f8
expect_fail "home-chat-v1 missing" "/boot-status does not list home-chat-v1 after 60 s"
[ "$(wc -l <"$FAKE_STATE/boot-polls.log")" -eq 12 ] || fail "boot-status must be polled 12 x 5 s"
ok "preflight / enable / version / backup / media / voice / doctor / home-chat-v1 failures are fatal"

echo "== time limits: each long step names its timeout =="
FAKE_HANG="hostchat enable --identity k2" run_step t1
expect_fail "enable timeout" "hostchat enable --identity k2 timed out after 20 min"
FAKE_HANG="hostchat backup" run_step t2
expect_fail "backup timeout" "hostchat backup timed out after 30 min"
FAKE_HANG="hostchat media install" run_step t3
expect_fail "media timeout" "hostchat media install timed out after 15 min"
ok "enable / backup / media install timeouts stop provisioning and say so"

echo "== 9c.8: the recorder never fails provisioning =="
FAKE_REC_PREFLIGHT='NOT ready\n  ok    os             Linux\n  fail  ram            4.0 GiB of RAM  (recording needs at least 8 GiB of RAM)\n' run_step r1
[ "$rc" -eq 0 ] || fail "a 4 GiB box must still provision: $out"
<<<"$out" grep -q 'Sew: recorder skipped (not ready: ram 4.0 GiB of RAM' || fail "4 GiB skip line: $out"
if <<<"$calls" grep -q 'recording install'; then fail "nothing is installed on a 4 GiB box: $calls"; fi
<<<"$out" grep -q 'SUMMARY=installed (Sew' || fail "Sew itself still provisions: $out"
ok "under 8 GiB: one skip line naming the check, no install, provisioning goes on"
FAKE_REC_PREFLIGHT='NOT ready\n  fail  fit            Sew needs 9000 MiB; this shared box gives them 5000 MiB  (free memory)\n' run_step r2
[ "$rc" -eq 0 ] && <<<"$out" grep -q 'recorder skipped (not ready: fit' || fail "the model doesn't fit: $out"
ok "the model doesn't fit: a skip line"
FAIL_REC_INSTALL=1 run_step r3
[ "$rc" -eq 0 ] || fail "a failed recorder install must not stop provisioning: $out"
<<<"$out" grep -q 'Sew: recorder not installed (hostchat recording install exit 3' || fail "install failure line: $out"
ok "a failed install: a line, provisioning goes on"
K2_SKIP_RECORDER=1 run_step r4
[ "$rc" -eq 0 ] && <<<"$out" grep -q 'Sew: recorder skipped (K2_SKIP_RECORDER=1)' || fail "K2_SKIP_RECORDER: $out"
if <<<"$calls" grep -q 'hostchat recording'; then fail "K2_SKIP_RECORDER=1 runs no recorder step: $calls"; fi
ok "K2_SKIP_RECORDER=1 skips 9c.8"
grep -q '^#   K2_SKIP_RECORDER ' "$PROVISION" && grep -q 'echo "  recorder:  ${REC_LINE}"' "$PROVISION" || fail "K2_SKIP_RECORDER documented, summary prints the recorder line"
ok "K2_SKIP_RECORDER documented; summary prints the recorder line"

echo "== opt-out and legacy daemon: skipped, nothing run =="
K2_SKIP_SEW=1 run_step s1
[ "$rc" -eq 0 ] && <<<"$out" grep -q 'SUMMARY=skipped (K2_SKIP_SEW=1)' || fail "opt-out: $out"
[ -z "$calls" ] || fail "opt-out must run nothing: $calls"
ROOT_HELPER_MODE=legacy run_step s2
[ "$rc" -eq 0 ] && <<<"$out" grep -q 'SUMMARY=skipped (no root helper)' || fail "legacy: $out"
[ -z "$calls" ] || fail "legacy must run nothing: $calls"
ok "K2_SKIP_SEW=1 and a legacy daemon skip with a line"

echo ""
echo "PASS: provision Sew ($pass checks)"
