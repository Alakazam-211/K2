#!/usr/bin/env bash
# Hermetic test for step 7a of scripts/provision-k2-server.sh (the hosted-mail
# root helper). provision-k2-server.sh has no dry-run mode and must never run
# for real here, so this test:
#   1. checks the script's shape statically (the step exists, sits after the
#      daemon install and before the bake exit, adds no sudo policy), and
#   2. cuts the 7a block out of the script and runs it alone against a temp
#      K2_INSTALL_ROOT, with a fake daemon, a fake install-mail-helper.sh and
#      PATH shims for uname/stat/visudo/timeout. No root, no network.
# Run with: bash tests/cli/provision_mail_helper.sh
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
L_DAEMON="$(line_of 'sh "$INSTALLER"')"
L_MAIL="$(line_of '# ── 7a. mail root helper')"
L_BAKE_EXIT="$(line_of 'bake complete')"
[ -n "$L_DAEMON" ] && [ -n "$L_MAIL" ] && [ -n "$L_BAKE_EXIT" ] || fail "markers not found"
[ "$L_DAEMON" -lt "$L_MAIL" ] || fail "mail helper step must come after the daemon install"
[ "$L_MAIL" -lt "$L_BAKE_EXIT" ] || fail "mail helper step must run before the bake exit (bake images get it too)"
ok "step 7a runs after the daemon install and before the bake exit"
BLOCK="$(sed -n '/^# ── 7a\. mail root helper/,/^# ── 7b\./p' "$PROVISION")"
printf '%s' "$BLOCK" | grep -q 'install-mail-helper.sh' || fail "7a must use install-mail-helper.sh"
printf '%s' "$BLOCK" | grep -q -- '--version "$DAEMON_VERSION"' || fail "7a must pass the daemon version"
printf '%s' "$BLOCK" | grep -q 'releases/download/v${DAEMON_VERSION}/install-mail-helper.sh' \
	|| fail "7a must fetch the installer from the same-version release"
ok "uses install-mail-helper.sh at the daemon's version"
if printf '%s' "$BLOCK" | grep -q 'NOPASSWD'; then fail "7a must not write sudo policy itself"; fi
[ "$(grep -c 'NOPASSWD' "$PROVISION")" = "1" ] || fail "provision must keep exactly one NOPASSWD line (k2-pg-helper)"
ok "no new sudo policy in the provisioner"
if printf '%s\n' "$BLOCK" | grep -Ev '^[[:space:]]*#' | grep -Eq 'hostmail|k2 dns|/cli/mail|/cli/dns'; then fail "7a must not enable mail or touch DNS"; fi
ok "does not enable mail or touch DNS"

WORK="$(mktemp -d -t k2-provision-mail-helper-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
mkdir -p "$SHIMS"
REAL_STAT="$(command -v stat)"

cat >"$SHIMS/uname" <<'SH'
#!/bin/sh
case "${1:-}" in -s) echo "${SHIM_OS:-Linux}" ;; -m) echo x86_64 ;; *) echo "${SHIM_OS:-Linux}" ;; esac
SH
cat >"$SHIMS/timeout" <<'SH'
#!/bin/sh
shift
exec "$@"
SH
cat >"$SHIMS/visudo" <<SH
#!/bin/sh
echo "visudo \$*" >>"$WORK/visudo.log"
[ "\${VISUDO_FAIL:-0}" = "1" ] && { echo "parse error" >&2; exit 1; }
exit 0
SH
cat >"$SHIMS/stat" <<SH
#!/bin/sh
# GNU \`stat -c '%U:%G %a' f\` -> <owner> <octal mode>; owner from SHIM_OWNER.
if [ "\$1" = "-c" ]; then
    mode=\$("$REAL_STAT" -c '%a' "\$3" 2>/dev/null || "$REAL_STAT" -f '%Lp' "\$3")
    echo "\${SHIM_OWNER:-root:root} \$mode"
    exit 0
fi
exec "$REAL_STAT" "\$@"
SH
chmod +x "$SHIMS"/*

# Fake daemon home + fake script dir holding a fake installer.
K2_HOME_T="$WORK/home/k2"
mkdir -p "$K2_HOME_T/.local/bin"
cat >"$K2_HOME_T/.local/bin/k2-daemon" <<'SH'
#!/bin/sh
[ "$1" = "--version" ] && echo "k2-daemon ${FAKE_DAEMON_VERSION-0.45.0}"
SH
chmod +x "$K2_HOME_T/.local/bin/k2-daemon"
SD="$WORK/scripts"
mkdir -p "$SD"
cat >"$SD/install-mail-helper.sh" <<SH
#!/usr/bin/env bash
# Fake installer: record argv, then either fail or lay down the two files.
echo "\$*" >>"$WORK/installer.log"
[ "\${FAKE_INSTALL_RC:-0}" = "0" ] || { echo "fake installer failing" >&2; exit "\$FAKE_INSTALL_RC"; }
mkdir -p "\$K2_INSTALL_ROOT/usr/local/libexec" "\$K2_INSTALL_ROOT/etc/sudoers.d"
printf 'helper' >"\$K2_INSTALL_ROOT/usr/local/libexec/k2-mail-helper"
chmod "\${FAKE_HELPER_MODE:-0755}" "\$K2_INSTALL_ROOT/usr/local/libexec/k2-mail-helper"
printf 'k2 line' >"\$K2_INSTALL_ROOT/etc/sudoers.d/k2-mail-helper"
chmod "\${FAKE_SUDOERS_MODE:-0440}" "\$K2_INSTALL_ROOT/etc/sudoers.d/k2-mail-helper"
SH

STEP="$WORK/step.sh"
{
	echo 'set -euo pipefail'
	echo 'log() { printf "[provision] %s\n" "$*"; }'
	echo 'die() { printf "[provision] ERROR: %s\n" "$*" >&2; exit 1; }'
	printf '%s\n' "$BLOCK"
	echo 'echo "SUMMARY=${MAIL_HELPER_LINE}"'
} >"$STEP"

run_step() {
	# run_step <dest-root>; sets out/rc. Extra env comes from the caller.
	local dest="$1"
	rm -f "$WORK/installer.log" "$WORK/visudo.log"
	set +e
	out="$(PATH="$SHIMS:$PATH" K2_INSTALL_ROOT="$dest" K2_HOME="$K2_HOME_T" \
		SCRIPT_DIR="$SD" K2_RUN_USER="${K2_RUN_USER:-k2}" K2_VERSION="${K2_VERSION:-}" \
		RAW_BASE="file:///nonexistent" bash "$STEP" 2>&1)"
	rc=$?
	set -e
}

echo "== success: installs at the daemon's version, verifies, prints the line =="
run_step "$WORK/box1"
[ "$rc" -eq 0 ] || fail "success exit $rc: $out"
printf '%s\n' "$out" | sed 's/^/    | /'
grep -qx -- '--version 0.45.0 --user k2' "$WORK/installer.log" || fail "installer argv: $(cat "$WORK/installer.log")"
ok "installer called with --version 0.45.0 --user k2"
grep -qx 'visudo -c' "$WORK/visudo.log" || fail "visudo -c never ran"
ok "visudo -c ran after install"
printf '%s' "$out" | grep -q 'mail helper: installed (v0.45.0)' || fail "missing 'mail helper: installed (v0.45.0)': $out"
printf '%s' "$out" | grep -q 'SUMMARY=installed (v0.45.0)' || fail "summary var: $out"
ok "prints 'mail helper: installed (v0.45.0)'"

echo "== installer fails: provisioning stops with a clear message =="
FAKE_INSTALL_RC=7 run_step "$WORK/box2"
[ "$rc" -ne 0 ] || fail "installer failure must fail the step: $out"
printf '%s' "$out" | grep -q 'mail helper: install-mail-helper.sh --version 0.45.0 failed' || fail "message: $out"
printf '%s' "$out" | grep -q 'provisioning stopped' || fail "message: $out"
if printf '%s' "$out" | grep -q 'SUMMARY='; then fail "must not continue past a failed install"; fi
ok "installer failure is fatal and says so"

echo "== wrong helper mode / owner, wrong sudoers mode, visudo -c failure: fatal =="
FAKE_HELPER_MODE=0775 run_step "$WORK/box3"
[ "$rc" -ne 0 ] || fail "helper 0775 must fail"
printf '%s' "$out" | grep -q "expected 'root:root 755'" || fail "helper mode message: $out"
ok "helper not 0755 is fatal"
SHIM_OWNER=k2:k2 run_step "$WORK/box4"
[ "$rc" -ne 0 ] || fail "helper owned by k2 must fail"
printf '%s' "$out" | grep -q "is 'k2:k2 755'" || fail "owner message: $out"
ok "helper not root:root is fatal"
FAKE_SUDOERS_MODE=0644 run_step "$WORK/box5"
[ "$rc" -ne 0 ] || fail "sudoers 0644 must fail"
printf '%s' "$out" | grep -q "expected 'root:root 440'" || fail "sudoers mode message: $out"
ok "sudoers not 0440 is fatal"
VISUDO_FAIL=1 run_step "$WORK/box6"
[ "$rc" -ne 0 ] || fail "visudo -c failure must fail"
printf '%s' "$out" | grep -q 'visudo -c fails' || fail "visudo message: $out"
ok "visudo -c failure is fatal"

echo "== same version guard + unreadable daemon version =="
K2_VERSION=0.44.4 run_step "$WORK/box7"
[ "$rc" -ne 0 ] || fail "K2_VERSION mismatch must fail"
printf '%s' "$out" | grep -q 'must match the daemon' || fail "mismatch message: $out"
ok "K2_VERSION != installed daemon is fatal"
K2_VERSION=v0.45.0 run_step "$WORK/box8"
[ "$rc" -eq 0 ] || fail "K2_VERSION=v0.45.0 should match 0.45.0: $out"
ok "K2_VERSION with a v prefix matches"
FAKE_DAEMON_VERSION="" run_step "$WORK/box9"
# The fake daemon prints "k2-daemon " with no version -> unreadable.
[ "$rc" -ne 0 ] || fail "unreadable daemon version must fail: $out"
printf '%s' "$out" | grep -q 'could not read the installed daemon version' || fail "message: $out"
ok "unreadable daemon version is fatal"

echo "== not Linux: skipped with a note, nothing run =="
SHIM_OS=Darwin run_step "$WORK/box10"
[ "$rc" -eq 0 ] || fail "non-Linux skip exit $rc: $out"
printf '%s' "$out" | grep -q 'mail helper: skipped' || fail "skip note: $out"
[ ! -e "$WORK/installer.log" ] || fail "installer must not run off Linux"
ok "non-Linux skips with a note"

echo ""
echo "PASS: provision mail helper ($pass checks)"
