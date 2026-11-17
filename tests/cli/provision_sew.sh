#!/usr/bin/env bash
# Hermetic test for step 9c (Sew chat + voice, Home chat, voice rooms) of
# scripts/provision-k2-server.sh. The provisioner has no dry-run mode and
# must never run for real here, so this test:
#   1. checks the script's shape statically (9c sits after daemon readiness,
#      the root helper check and the owner user, before the callback; the
#      CLI runs as the daemon user with a clean env; K2_SKIP_SEW is
#      documented; the summary prints the line), and
#   2. cuts the 9c block out of the script and runs it alone with a fake
#      `k2` (k2_as_daemon_user stub) and PATH shims for curl/sleep.
#      No root, no network, no daemon.
# Run with: bash tests/cli/provision_sew.sh
# Literal $… in single quotes and A && B || fail are intended here.
# shellcheck disable=SC2015,SC2016,SC2028
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
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

WORK="$(mktemp -d -t k2-provision-sew-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
mkdir -p "$SHIMS"
cat >"$SHIMS/curl" <<'SH'
#!/bin/sh
# Only /boot-status is fetched in 9c.
[ -n "${FAKE_BOOT+x}" ] || FAKE_BOOT='{"phase":"ready","features":["home-chat-v1","voice-rooms-v1"]}'
printf '%s\n' "$FAKE_BOOT"
SH
cat >"$SHIMS/sleep" <<'SH'
#!/bin/sh
exit 0
SH
cat >"$SHIMS/k2" <<'SH'
#!/usr/bin/env bash
# Fake CLI. State in $FAKE_STATE; every call appended to calls.log.
echo "$*" >>"$FAKE_STATE/calls.log"
case "$*" in
	"hostchat preflight") printf '%b' "${FAKE_PREFLIGHT-ready\nall checks pass\n}" ;;
	"hostchat status")
		[ -e "$FAKE_STATE/enabled" ] && echo "identity: k2" || echo "identity: none"
		[ -e "$FAKE_STATE/voice" ] && echo "voice: running" || echo "voice: off"
		;;
	"hostchat enable --identity k2")
		[ "${FAIL_ENABLE:-0}" = 1 ] && { echo "enable boom" >&2; exit 3; }
		touch "$FAKE_STATE/enabled"; echo "linked" ;;
	"hostchat media install")
		[ "${FAIL_MEDIA:-0}" = 1 ] && { echo "media boom" >&2; exit 3; }
		[ "${MEDIA_NOOP:-0}" = 1 ] || touch "$FAKE_STATE/voice"; echo "installed" ;;
	"settings set homeChat true"|"settings set voiceRoomsPreview true") echo "ok" ;;
	"hostchat backup")
		[ "${FAIL_BACKUP:-0}" = 1 ] && { echo "backup boom" >&2; exit 3; }
		echo '{"id":"b1"}' ;;
	"hostchat doctor --json")
		[ -n "${FAKE_DOCTOR+x}" ] || FAKE_DOCTOR='{"verdict":{"failed":[]}}\n'
		printf '%b' "$FAKE_DOCTOR"
		[ "${DOCTOR_RC:-0}" = 0 ] || exit "$DOCTOR_RC" ;;
	*) echo "fake k2: unexpected $*" >&2; exit 99 ;;
esac
SH
chmod +x "$SHIMS"/*

STEP="$WORK/step.sh"
{
	echo 'set -euo pipefail'
	echo 'log() { printf "[provision] %s\n" "$*"; }'
	echo 'die() { printf "[provision] ERROR: %s\n" "$*" >&2; exit 1; }'
	echo 'k2_as_daemon_user() { k2 "$@"; }'
	printf '%s\n' "$BLOCK"
	echo 'echo "SUMMARY=${SEW_LINE}"'
} >"$STEP"

run_step() { # <box>
	export FAKE_STATE="$WORK/$1"
	rm -rf "$FAKE_STATE"; mkdir -p "$FAKE_STATE"; : >"$FAKE_STATE/calls.log"
	[ "${PRE_ENABLED:-0}" = 1 ] && touch "$FAKE_STATE/enabled"
	[ "${PRE_VOICE:-0}" = 1 ] && touch "$FAKE_STATE/voice"
	set +e
	out="$(PATH="$SHIMS:$PATH" K2_RUN_USER=k2 BASE="http://127.0.0.1:1" \
		ROOT_HELPER_MODE="${ROOT_HELPER_MODE-root}" MAIL_HELPER_LINE="x" bash "$STEP" 2>&1)"
	rc=$?
	set -e
	calls="$(cat "$FAKE_STATE/calls.log")"
}
mutating() { <<<"$calls" grep -v '^hostchat status$' || true; }

echo "== fresh box: all seven steps in order, features checked =="
run_step box1
[ "$rc" -eq 0 ] || fail "success exit $rc: $out"
printf '%s\n' "$out" | sed 's/^/    | /'
want="hostchat preflight
hostchat enable --identity k2
hostchat media install
settings set homeChat true
settings set voiceRoomsPreview true
hostchat backup
hostchat doctor --json"
[ "$(mutating)" = "$want" ] || fail "call order: $(mutating)"
ok "preflight → enable → media install → homeChat → voiceRoomsPreview → backup → doctor"
<<<"$out" grep -q 'SUMMARY=installed (identity k2, voice running, home-chat-v1, voice-rooms-v1)' || fail "summary: $out"
ok "summary line names identity, voice and both features"

echo "== re-run on a linked box: enable and media install are skipped =="
PRE_ENABLED=1 PRE_VOICE=1 run_step box2
[ "$rc" -eq 0 ] || fail "re-run exit $rc: $out"
if <<<"$calls" grep -Eq 'enable|media install'; then fail "re-run must not enable/install again: $calls"; fi
ok "idempotent re-run"

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
expect_fail "enable" "hostchat enable --identity k2"
<<<"$out" grep -q 'enable boom' || fail "enable output must be shown: $out"
FAIL_MEDIA=1 run_step f3
expect_fail "media install" "hostchat media install"
MEDIA_NOOP=1 run_step f4
expect_fail "voice not running" "hostchat media install (status does not show voice: running)"
FAIL_BACKUP=1 run_step f5
expect_fail "backup" "hostchat backup"
FAKE_DOCTOR='{\n  "verdict": {\n    "failed": ["voice-turn"]\n  }\n}\n' DOCTOR_RC=1 run_step f6
expect_fail "doctor verdict" 'hostchat doctor (verdict.failed: \["voice-turn"\])'
FAKE_DOCTOR='progress...\n{"verdict":{"failed":[]}}\n' run_step f7
[ "$rc" -eq 0 ] || fail "doctor JSON on the last line must parse: $out"
FAKE_DOCTOR='garbage\n' run_step f8
expect_fail "unreadable doctor" "hostchat doctor --json (unreadable verdict)"
FAKE_BOOT='{"features":["voice-rooms-v1"]}' run_step f9
expect_fail "home-chat-v1 missing" "/boot-status does not list home-chat-v1"
ok "preflight / enable / media / voice / backup / doctor / home-chat-v1 failures are fatal"

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
