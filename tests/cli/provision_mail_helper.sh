#!/usr/bin/env bash
# Hermetic test for step 7a (the root helper) and 7b (db sidecar bake) of
# scripts/provision-k2-server.sh. provision-k2-server.sh has no dry-run mode
# and must never run for real here, so this test:
#   1. checks the script's shape statically (the step exists, sits after the
#      daemon install and before the bake exit, adds no sudo policy, 7b no
#      longer installs k2-pg-helper or writes its sudoers), and
#   2. cuts the 7a block out of the script and runs it alone against a temp
#      K2_INSTALL_ROOT, with a fake daemon, helper files as step 4 leaves
#      them and PATH shims for uname/stat/visudo/timeout. No root, no network.
# Step 4 (install-daemon.sh installs the helper, once) runs for real in
# tests/packaging/install-daemon-root-helper.sh.
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
L_HELPER="$(line_of '# ── 7a. root helper')"
L_BAKE_EXIT="$(line_of 'bake complete')"
[ -n "$L_DAEMON" ] && [ -n "$L_HELPER" ] && [ -n "$L_BAKE_EXIT" ] || fail "markers not found"
[ "$L_DAEMON" -lt "$L_HELPER" ] || fail "root helper step must come after the daemon install"
[ "$L_HELPER" -lt "$L_BAKE_EXIT" ] || fail "root helper step must run before the bake exit (bake images get it too)"
ok "step 7a runs after the daemon install and before the bake exit"
BLOCK="$(sed -n '/^# ── 7a\. root helper/,/^# ── 7b\./p' "$PROVISION")"
FETCH_BLOCK="$(sed -n '/^# ── GitHub fetch: begin/,/^# ── GitHub fetch: end/p' "$PROVISION")"
[ -n "$FETCH_BLOCK" ] || fail "the GitHub fetch block not found"
STEP4="$(sed -n '/^# ── 4\. daemon binary/,/^# ── 5\./p' "$PROVISION")"
[ -n "$STEP4" ] || fail "step 4 not found"
# One install path: step 4's install-daemon.sh installs the helper (through
# install-root-helper.sh, at the daemon's version); 7a only checks it.
if <<<"$(<<<"$BLOCK" grep -Ev '^[[:space:]]*#')" grep -Eq 'ROOT_HELPER_INSTALLER|ROOT_HELPER_ARGS|install-root-helper\.sh" |\$INSTALLER'; then
	fail "7a must not run an installer again (step 4 installs the helper)"
fi
<<<"$STEP4" grep -q -- '--user "$K2_RUN_USER" --no-service --version "$K2_VERSION"' \
	|| fail "step 4 must run install-daemon.sh as root for the daemon user at K2_VERSION"
<<<"$STEP4" grep -q -- '--unit "${K2_UNIT_NAME}.service" --scope system' \
	|| fail "step 4 must name this unit + system scope for daemon.toml (RH15)"
<<<"$STEP4" grep -q -- '--root-helper-file "$ROOT_HELPER_SRC"' || fail "step 4 passes a local helper build on"
if <<<"$STEP4" grep -q 'sudo -u'; then fail "step 4 must not drop to the daemon user itself (install-daemon.sh --user does)"; fi
grep -q 'install-root-helper.sh' "$ROOT/scripts/install-daemon.sh" || fail "install-daemon.sh must use install-root-helper.sh"
grep -q 'RH_INSTALLER="${RELEASE_BASE%/}/v$MVER/install-root-helper.sh"' "$ROOT/scripts/install-daemon.sh" \
	|| fail "install-daemon.sh must fetch the installer from the same-version release"
ok "step 4 installs the helper with the daemon (install-daemon.sh, this unit, system scope); 7a installs nothing"
if <<<"$BLOCK" grep -q 'NOPASSWD'; then fail "7a must not write sudo policy itself"; fi
[ "$(grep -c 'NOPASSWD' "$PROVISION")" = "1" ] || fail "only the legacy (pre-0.45.4) pg branch writes a sudoers line"
grep -q 'visudo -cf "$PG_SUDO_TMP"' "$PROVISION" || fail "the legacy pg sudoers line must be visudo-checked"
if grep -Eq 'cat > /etc/sudoers' "$PROVISION"; then fail "an unchecked sudoers write is back (RH11)"; fi
ok "no sudo policy in 7a; the legacy pg line is visudo-checked"
DB_BLOCK="$(sed -n '/^# ── 7b\./,/bake complete/p' "$PROVISION")"
ROOT_DB="$(printf '%s\n' "$DB_BLOCK" | sed -n '/^\telse$/,/^\tfi$/p')"
[ -n "$ROOT_DB" ] || fail "7b root-helper branch not found"
if <<<"$(<<<"$ROOT_DB" grep -Ev '^[[:space:]]*#')" grep -Eq 'k2-pg-helper|sudoers'; then
	fail "7b (root helper) must not install k2-pg-helper or write a sudoers file (RH11)"
fi
<<<"$DB_BLOCK" grep -q 'k2-root-helper pg install-ram-fence' || fail "7b fences RAM through the root helper"
ok "7b: no pg helper copy, no unchecked sudoers, RAM fence via k2-root-helper"
# Customer boxes: an existing cluster is adopted — the check runs before apt and
# the adopt branch never installs, inits, rewrites config or fences.
L_PGEXIST="$(line_of 'PG_EXISTING=""')"
L_APT="$(line_of 'apt-get install -y -qq postgresql')"
[ -n "$L_PGEXIST" ] && [ -n "$L_APT" ] && [ "$L_PGEXIST" -lt "$L_APT" ] || fail "7b must look for an existing cluster before apt"
# The adopt branch ends at the next branch of that if (0.46.3: the PG 17
# helper branch, then the distro branch).
ADOPT="$(sed -n '/^if \[ "\$WITH_DB" = 1 \] && \[ -n "\$PG_EXISTING" \]; then$/,/^elif /p' "$PROVISION")"
[ -n "$ADOPT" ] || fail "7b adopt branch not found"
if <<<"$(<<<"$ADOPT" grep -Ev '^[[:space:]]*(#|log )')" grep -Eq 'apt-get|initdb|pg_createcluster|listen_addresses|sed -i|install-ram-fence|systemctl'; then
	fail "the adopt branch must not install, init, reconfigure or fence an existing cluster"
fi
grep -q 'ROOT_HELPER_FROM="0.45.4"' "$PROVISION" || fail "the root helper ships from 0.45.4"
ok "7b adopts an existing Postgres cluster read-only (no re-init, no config change)"
if <<<"$(<<<"$BLOCK" grep -Ev '^[[:space:]]*#')" grep -Eq 'hostmail|k2 dns|/cli/mail|/cli/dns'; then fail "7a must not enable mail or touch DNS"; fi
ok "does not enable mail or touch DNS"
# Root never executes the daemon user's binary: every line that runs the
# installed k2-daemon must drop to the daemon user first.
# The lines that start the daemon, collected first: the last grep stops at
# its first match, and under pipefail that failed the stage before it.
daemon_starts="$(<<<"$BLOCK" grep -Ev '^[[:space:]]*#' | grep -F '/.local/bin/k2-daemon"' || true)"
if [ -n "$daemon_starts" ] && <<<"$daemon_starts" grep -vq 'runuser -u "$K2_RUN_USER" --'; then
	fail "7a runs the daemon user's k2-daemon without dropping to that user"
fi
if grep -Ev '^[[:space:]]*#' "$PROVISION" | grep -F '/.local/bin/k2-daemon"' | grep -F -- '--version' | grep -vq 'runuser -u'; then
	fail "the provisioner runs k2-daemon --version as root somewhere"
fi
ok "the installed daemon only ever runs as the daemon user"
L_RESOLVE="$(line_of 'K2_VERSION must look like')"
[ -n "$L_RESOLVE" ] && [ "$L_RESOLVE" -lt "$L_DAEMON" ] || fail "K2_VERSION must be resolved and validated before step 4"
ok "root resolves and validates K2_VERSION before the daemon install"
if grep -Ev '^[[:space:]]*#' "$PROVISION" | grep -Eq '/tmp/k2-install'; then
	fail "the provisioner downloads an installer to a fixed /tmp name"
fi
ok "no fixed /tmp installer paths"

WORK="$(mktemp -d -t k2-provision-root-helper-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
mkdir -p "$SHIMS"
REAL_STAT="$(command -v stat)"

cat >"$SHIMS/uname" <<'SH'
#!/bin/sh
case "${1:-}" in -s) echo "${SHIM_OS:-Linux}" ;; -m) echo x86_64 ;; *) echo "${SHIM_OS:-Linux}" ;; esac
SH
cat >"$SHIMS/runuser" <<SH
#!/bin/sh
# runuser -u <user> -- cmd...: record the user, run cmd as this test user.
[ "\$1" = "-u" ] && [ "\$3" = "--" ] || { echo "runuser shim: bad argv \$*" >&2; exit 97; }
echo "\$2" >>"$WORK/runuser.log"
shift 3
SHIM_VIA_RUNUSER=1 exec "\$@"
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
cat >"$SHIMS/curl" <<SH
#!/bin/sh
# curl … -o <dest> <url>: serve https://github.com/…/download/<path> from the mirror.
dest=""; url=""
while [ \$# -gt 0 ]; do
    case "\$1" in -o) dest="\$2"; shift 2 ;; -*) shift ;; *) url="\$1"; shift ;; esac
done
src="$WORK/rel/\${url##*/releases/download/}"
[ -f "\$src" ] || exit 22
cp "\$src" "\$dest"
SH
chmod +x "$SHIMS"/*
mkdir -p "$WORK/rel/v0.45.0"
cat >"$WORK/rel/v0.45.0/install-mail-helper.sh" <<SH
#!/usr/bin/env bash
echo "legacy \$*" >>"$WORK/installer.log"
mkdir -p "\$K2_INSTALL_ROOT/usr/local/libexec" "\$K2_INSTALL_ROOT/etc/sudoers.d"
printf 'mail helper' >"\$K2_INSTALL_ROOT/usr/local/libexec/k2-mail-helper"
chmod 0755 "\$K2_INSTALL_ROOT/usr/local/libexec/k2-mail-helper"
printf 'k2 line' >"\$K2_INSTALL_ROOT/etc/sudoers.d/k2-mail-helper"
chmod 0440 "\$K2_INSTALL_ROOT/etc/sudoers.d/k2-mail-helper"
SH

# Fake daemon home, and the helper files as step 4's install-daemon.sh leaves
# them (lay_helper). Any installer 7a ran would land in installer.log.
K2_HOME_T="$WORK/home/k2"
mkdir -p "$K2_HOME_T/.local/bin"
cat >"$K2_HOME_T/.local/bin/k2-daemon" <<SH
#!/bin/sh
echo "\${SHIM_VIA_RUNUSER:-direct}" >>"$WORK/daemon-runs.log"
[ "\$1" = "--version" ] && echo "k2-daemon \${FAKE_DAEMON_VERSION-0.45.5}"
SH
chmod +x "$K2_HOME_T/.local/bin/k2-daemon"
SD="$WORK/scripts"
mkdir -p "$SD"
cat >"$SD/install-root-helper.sh" <<SH
#!/usr/bin/env bash
echo "\$*" >>"$WORK/installer.log"
SH
lay_helper() { # <box>: helper (build marker FAKE_HELPER_VERSION), sudoers, daemon.toml
	local d="$1"
	mkdir -p "$d/usr/local/libexec" "$d/etc/sudoers.d" "$d/etc/k2-root-helper"
	rm -f "$d/usr/local/libexec/k2-root-helper" "$d/etc/sudoers.d/k2-root-helper" "$d/etc/k2-root-helper/daemon.toml"
	printf '\177ELF\0K2_ROOT_HELPER_BUILD v=%s proto=13 arch=x86_64;\0' "${FAKE_HELPER_VERSION:-0.45.5}" \
		>"$d/usr/local/libexec/k2-root-helper"
	chmod "${FAKE_HELPER_MODE:-0755}" "$d/usr/local/libexec/k2-root-helper"
	printf 'k2 line' >"$d/etc/sudoers.d/k2-root-helper"
	chmod "${FAKE_SUDOERS_MODE:-0440}" "$d/etc/sudoers.d/k2-root-helper"
	printf '[[daemon]]' >"$d/etc/k2-root-helper/daemon.toml"
	chmod "${FAKE_TOML_MODE:-0644}" "$d/etc/k2-root-helper/daemon.toml"
}

STEP="$WORK/step.sh"
{
	echo 'set -euo pipefail'
	echo 'log() { printf "[provision] %s\n" "$*"; }'
	echo 'die() { printf "[provision] ERROR: %s\n" "$*" >&2; exit 1; }'
	# The legacy path downloads through the script's GitHub fetch block.
	printf '%s\n' "$FETCH_BLOCK" "$BLOCK"
	echo 'echo "SUMMARY=${MAIL_HELPER_LINE}"'
} >"$STEP"

run_step() {
	local dest="$1"
	rm -f "$WORK/installer.log" "$WORK/visudo.log" "$WORK/runuser.log" "$WORK/daemon-runs.log"
	[ "${NO_LAY:-0}" = 1 ] || lay_helper "$dest"
	set +e
	out="$(PATH="$SHIMS:$PATH" K2_INSTALL_ROOT="$dest" K2_HOME="$K2_HOME_T" \
		SCRIPT_DIR="$SD" K2_RUN_USER="${K2_RUN_USER:-k2}" K2_UNIT_NAME="${K2_UNIT_NAME:-k2-daemon}" \
		K2_VERSION="${K2_VERSION-0.45.5}" RAW_BASE="file:///nonexistent" bash "$STEP" 2>&1)"
	rc=$?
	set -e
}

echo "== success: checks the helper step 4 installed, installs nothing, prints the line =="
run_step "$WORK/box1"
[ "$rc" -eq 0 ] || fail "success exit $rc: $out"
printf '%s\n' "$out" | sed 's/^/    | /'
[ ! -e "$WORK/installer.log" ] || fail "7a ran an installer again: $(cat "$WORK/installer.log")"
ok "no second install in 7a"
grep -qx 'visudo -c' "$WORK/visudo.log" || fail "visudo -c never ran"
ok "visudo -c ran"
<<<"$out" grep -q 'root helper: installed (v0.45.5)' || fail "missing 'root helper: installed (v0.45.5)': $out"
<<<"$out" grep -q 'SUMMARY=installed (v0.45.5)' || fail "summary var: $out"
ok "prints 'root helper: installed (v0.45.5)'"
grep -qx k2 "$WORK/runuser.log" || fail "the version cross-check must run as k2"
if grep -qx direct "$WORK/daemon-runs.log"; then fail "k2-daemon was executed without dropping to k2"; fi
ok "the installed daemon ran only as k2 (runuser), never as the caller"
FAKE_HELPER_VERSION=0.46.0 run_step "$WORK/box1c"
[ "$rc" -eq 0 ] && <<<"$out" grep -q 'SUMMARY=installed (v0.46.0)' || fail "a newer helper is fine: $out"
ok "a helper newer than the daemon passes"

echo "== step 4 left no helper, or an older one: provisioning stops with a clear message =="
NO_LAY=1 run_step "$WORK/box2"
[ "$rc" -ne 0 ] || fail "a missing helper must fail the step: $out"
<<<"$out" grep -q 'k2-root-helper is missing after install' || fail "message: $out"
<<<"$out" grep -q 'provisioning stopped' || fail "message: $out"
if <<<"$out" grep -q 'SUMMARY='; then fail "must not continue past a missing helper"; fi
FAKE_HELPER_VERSION=0.45.4 run_step "$WORK/box2b"
[ "$rc" -ne 0 ] && <<<"$out" grep -q 'the root helper is v0.45.4, older than the daemon v0.45.5' || fail "older helper: $out"
ok "missing or older helper is fatal and says so"
mkdir -p "$WORK/box2c/usr/local/libexec" "$WORK/box2c/usr/lib/k2"
printf 'pkg' >"$WORK/box2c/usr/lib/k2/k2-root-helper"
ln -s "$WORK/box2c/usr/lib/k2/k2-root-helper" "$WORK/box2c/usr/local/libexec/k2-root-helper"
NO_LAY=1 run_step "$WORK/box2c"
[ "$rc" -eq 0 ] && <<<"$out" grep -q 'SUMMARY=left alone (managed by the package manager)' || fail "package-owned: $out"
ok "a package-owned (symlinked) helper is left alone"

echo "== wrong modes / owner, visudo -c failure: fatal =="
FAKE_HELPER_MODE=0775 run_step "$WORK/box3"
[ "$rc" -ne 0 ] || fail "helper 0775 must fail"
<<<"$out" grep -q "expected 'root:root 755'" || fail "helper mode message: $out"
SHIM_OWNER=k2:k2 run_step "$WORK/box4"
[ "$rc" -ne 0 ] || fail "helper owned by k2 must fail"
<<<"$out" grep -q "is 'k2:k2 755'" || fail "owner message: $out"
FAKE_SUDOERS_MODE=0644 run_step "$WORK/box5"
[ "$rc" -ne 0 ] || fail "sudoers 0644 must fail"
<<<"$out" grep -q "expected 'root:root 440'" || fail "sudoers mode message: $out"
FAKE_TOML_MODE=0666 run_step "$WORK/box5b"
[ "$rc" -ne 0 ] || fail "daemon.toml 0666 must fail"
<<<"$out" grep -q "expected 'root:root 644'" || fail "toml mode message: $out"
VISUDO_FAIL=1 run_step "$WORK/box6"
[ "$rc" -ne 0 ] || fail "visudo -c failure must fail"
<<<"$out" grep -q 'visudo -c fails' || fail "visudo message: $out"
ok "helper / owner / sudoers / daemon.toml modes and visudo -c are fatal"

echo "== same version guard + unreadable daemon version =="
K2_VERSION=0.44.4 run_step "$WORK/box7"
[ "$rc" -ne 0 ] || fail "K2_VERSION mismatch must fail"
<<<"$out" grep -q 'must match the daemon' || fail "mismatch message: $out"
K2_VERSION=v0.45.5 run_step "$WORK/box8"
[ "$rc" -eq 0 ] || fail "K2_VERSION=v0.45.5 should match 0.45.5: $out"
FAKE_DAEMON_VERSION="" run_step "$WORK/box9"
[ "$rc" -ne 0 ] || fail "unreadable daemon version must fail: $out"
<<<"$out" grep -q 'could not read the installed daemon version' || fail "message: $out"
ok "version guards"

echo "== K2_VERSION is required and must be plain x.y.z =="
for bad in "" "latest" "0.45" "0.45.5-rc1" '0.45.5;id' '0.45.5/../x'; do
	K2_VERSION="$bad" run_step "$WORK/box-badver"
	[ "$rc" -ne 0 ] || fail "K2_VERSION='$bad' must fail: $out"
	<<<"$out" grep -q 'plain x.y.z' || fail "K2_VERSION='$bad' message: $out"
	[ ! -e "$WORK/installer.log" ] || fail "K2_VERSION='$bad': installer must not run"
done
ok "missing or malformed K2_VERSION stops before anything runs"

echo "== daemon older than the root helper: that release's k2-mail-helper =="
NO_LAY=1 FAKE_DAEMON_VERSION=0.45.0 K2_VERSION=0.45.0 run_step "$WORK/box-legacy"
[ "$rc" -eq 0 ] || fail "legacy path: $out"
grep -qx -- 'legacy --version 0.45.0 --user k2' "$WORK/installer.log" || fail "legacy installer argv: $(cat "$WORK/installer.log")"
[ -f "$WORK/box-legacy/usr/local/libexec/k2-mail-helper" ] || fail "legacy helper not installed"
[ ! -e "$WORK/box-legacy/usr/local/libexec/k2-root-helper" ] || fail "no root helper for an old daemon"
<<<"$out" grep -q 'installed legacy k2-mail-helper (v0.45.0 predates the root helper)' || fail "legacy summary: $out"
ok "a pre-0.45.5 daemon (raw-main provisioner) still gets its own release's mail helper"

echo "== not Linux: skipped with a note, nothing run =="
NO_LAY=1 SHIM_OS=Darwin run_step "$WORK/box10"
[ "$rc" -eq 0 ] || fail "non-Linux skip exit $rc: $out"
<<<"$out" grep -q 'root helper: skipped' || fail "skip note: $out"
[ ! -e "$WORK/installer.log" ] || fail "installer must not run off Linux"
ok "non-Linux skips with a note"

echo ""
echo "PASS: provision root helper ($pass checks)"
