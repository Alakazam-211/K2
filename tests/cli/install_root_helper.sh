#!/usr/bin/env bash
# Hermetic test for scripts/install-root-helper.sh (+ the install-mail-helper.sh
# wrapper). No root, no network, no real /usr/local or /etc: K2_INSTALL_ROOT
# redirects every destination into a temp dir; PATH shims stand in for
# id/uname/install/stat/visudo/sudo/minisign. The minisign shim "verifies" a
# .sig carrying GOOD-SIG against a pubkey file carrying the test key line
# and prints its trusted comment (like `minisign -V -Q`). Release assets are
# a file:// mirror; fake ELFs carry real build-marker strings.
# Run with: bash tests/cli/install_root_helper.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INSTALLER="$ROOT/scripts/install-root-helper.sh"
WRAPPER="$ROOT/scripts/install-mail-helper.sh"
[ -f "$INSTALLER" ] || { echo "FAIL: missing $INSTALLER" >&2; exit 1; }

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

WORK="$(mktemp -d -t k2-install-root-helper-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
mkdir -p "$SHIMS"
REAL_INSTALL="$(command -v install)"
REAL_STAT="$(command -v stat)"
mode_of() { "$REAL_STAT" -c '%a' "$1" 2>/dev/null || "$REAL_STAT" -f '%Lp' "$1"; }
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

cat >"$SHIMS/id" <<'SH'
#!/bin/sh
if [ "${1:-}" = "-u" ]; then echo "${SHIM_UID:-0}"; exit 0; fi
exit 0
SH
cat >"$SHIMS/uname" <<'SH'
#!/bin/sh
case "${1:-}" in -s) echo "${SHIM_OS:-Linux}" ;; -m) echo "${SHIM_ARCH:-x86_64}" ;; *) echo Linux ;; esac
SH
cat >"$SHIMS/install" <<SH
#!/bin/sh
args=""
while [ \$# -gt 0 ]; do
    case "\$1" in
        -o|-g) echo "\$1 \$2" >>"$WORK/install-owner.log"; shift 2 ;;
        *) args="\$args
\$1"; shift ;;
    esac
done
IFS='
'
# shellcheck disable=SC2086
exec "$REAL_INSTALL" \$args
SH
cat >"$SHIMS/stat" <<SH
#!/bin/sh
if [ "\$1" = "-c" ]; then
    mode=\$("$REAL_STAT" -c '%a' "\$3" 2>/dev/null || "$REAL_STAT" -f '%Lp' "\$3")
    echo "root:root \$mode"
    exit 0
fi
exec "$REAL_STAT" "\$@"
SH
cat >"$SHIMS/visudo" <<SH
#!/bin/sh
echo "visudo \$*" >>"$WORK/visudo.log"
[ "\${VISUDO_FAIL:-0}" = "1" ] && { echo "parse error" >&2; exit 1; }
exit 0
SH
cat >"$SHIMS/sudo" <<'SH'
#!/bin/sh
echo "sudo must not run in the hermetic test" >&2
exit 99
SH
cat >"$SHIMS/runuser" <<'SH'
#!/bin/sh
echo "runuser must not run in the hermetic test" >&2
exit 99
SH
cat >"$SHIMS/minisign" <<'SH'
#!/bin/sh
# minisign -V -m <file> -p <pub> -x <sig> -Q
sig=""; pub=""
while [ $# -gt 0 ]; do
    case "$1" in -x) sig="$2"; shift 2 ;; -p) pub="$2"; shift 2 ;; *) shift ;; esac
done
grep -q "^${SHIM_GOOD_KEY:-RWTESTKEY}\$" "$pub" || exit 1
grep -q GOOD-SIG "$sig" || exit 1
sed -n 's/^trusted comment: //p' "$sig"
SH
if ! command -v sha256sum >/dev/null 2>&1; then
	cat >"$SHIMS/sha256sum" <<'SH'
#!/bin/sh
exec shasum -a 256 "$@"
SH
fi
chmod +x "$SHIMS"/*
export PATH="$SHIMS:$PATH"

MARK="K2_ROOT_HELPER_BUILD"
fake_elf() {
	# fake_elf <out> <version> <protocol> <arch> [extra-marker]
	{
		printf '\177ELF'
		printf 'padding\0'
		printf '%s v=%s proto=%s arch=%s;' "$MARK" "$2" "$3" "$4"
		[ -z "${5:-}" ] || printf '\0%s' "$5"
		printf '\0tail'
	} >"$1"
}
release() {
	# release <version> <protocol> [comment-override] [extra-marker] [marker-version]
	local v="$1" p="$2" dir="$WORK/rel/v$1" a="k2-root-helper-linux-x86_64"
	mkdir -p "$dir"
	fake_elf "$dir/$a" "${5:-$v}" "$p" x86_64 "${4:-}"
	printf 'untrusted comment: test\nGOOD-SIG\ntrusted comment: %s\nGLOBAL\n' \
		"${3:-file:$a version:$v protocol:$p}" >"$dir/$a.sig"
	(cd "$dir" && sha "$a" >"$a.sha256")
}

run() {
	# run <dest-root> [args...]; sets out/rc. TEST key trusted unless NOKEYS=1.
	local dest="$1"
	shift
	local keys="RWTESTKEY"
	[ "${NOKEYS:-0}" = 1 ] && keys=""
	set +e
	out="$(K2_INSTALL_ROOT="$dest" K2_RELEASE_BASE="file://$WORK/rel" \
		K2_ROOT_HELPER_TEST_PUBKEYS="$keys" bash "${SCRIPT:-$INSTALLER}" "$@" 2>&1)"
	rc=$?
	set -e
}

nothing_installed() {
	local dest="$1" label="$2" f
	[ ! -e "$dest/usr/local/libexec/k2-root-helper" ] || fail "$label: helper must not be installed"
	[ ! -e "$dest/etc/sudoers.d/k2-root-helper" ] || fail "$label: sudoers must not be written"
	[ ! -e "$dest/etc/k2-root-helper/daemon.toml" ] || fail "$label: daemon.toml must not be written"
	for f in "$dest"/usr/local/libexec/.k2-root-helper.* "$dest"/etc/sudoers.d/.k2-root-helper.*; do
		[ ! -e "$f" ] || fail "$label: staged file left behind: $f"
	done
}

expect_fail_with() {
	[ "$rc" -ne 0 ] || fail "$1: expected failure, got exit 0: $out"
	printf '%s' "$out" | grep -q -- "$2" || fail "$1: missing '$2' in: $out"
}

legacy() {
	mkdir -p "$1/usr/local/libexec" "$1/etc/sudoers.d"
	printf 'old pg helper' >"$1/usr/local/libexec/k2-pg-helper"
	printf 'k2 ALL=(root) NOPASSWD: /usr/local/libexec/k2-pg-helper\n' >"$1/etc/sudoers.d/k2-pg-helper"
	printf '\177ELF old mail helper' >"$1/usr/local/libexec/k2-mail-helper"
	printf 'k2 ALL=(root) NOPASSWD: /usr/local/libexec/k2-mail-helper\n' >"$1/etc/sudoers.d/k2-mail-helper"
}

release 0.45.5 4
release 0.45.6 4

echo "== fresh install from a verified release asset; pg legacy retired =="
D1="$WORK/box1"
legacy "$D1"
run "$D1" --version 0.45.5
[ "$rc" -eq 0 ] || fail "fresh install exit $rc: $out"
printf '%s\n' "$out" | sed 's/^/    | /'
cmp -s "$WORK/rel/v0.45.5/k2-root-helper-linux-x86_64" "$D1/usr/local/libexec/k2-root-helper" || fail "helper bytes"
[ "$(mode_of "$D1/usr/local/libexec/k2-root-helper")" = "755" ] || fail "helper mode"
grep -q -- "-o root" "$WORK/install-owner.log" || fail "install must ask for -o root"
ok "helper installed root:root 0755 with the release bytes"
expected="$(printf 'k2 ALL=(root) NOPASSWD: /usr/local/libexec/k2-root-helper\nDefaults:k2 !requiretty')"
[ "$(cat "$D1/etc/sudoers.d/k2-root-helper")" = "$expected" ] || fail "sudoers: $(cat "$D1/etc/sudoers.d/k2-root-helper")"
[ "$(mode_of "$D1/etc/sudoers.d/k2-root-helper")" = "440" ] || fail "sudoers mode"
grep -q -- "-cf" "$WORK/visudo.log" || fail "visudo -cf never ran"
ok "one sudoers line pair, 0440, visudo-checked"
grep -q '^user = "k2"$' "$D1/etc/k2-root-helper/daemon.toml" || fail "daemon.toml user"
grep -q '^unit = "k2-daemon.service"$' "$D1/etc/k2-root-helper/daemon.toml" || fail "daemon.toml unit"
grep -q '^scope = "system"$' "$D1/etc/k2-root-helper/daemon.toml" || fail "daemon.toml scope"
[ "$(mode_of "$D1/etc/k2-root-helper/daemon.toml")" = "644" ] || fail "daemon.toml mode"
ok "daemon.toml names the daemon (0644)"
[ ! -e "$D1/usr/local/libexec/k2-pg-helper" ] && [ ! -e "$D1/etc/sudoers.d/k2-pg-helper" ] || fail "pg legacy not retired"
grep -q -- "visudo -c" "$WORK/visudo.log" || fail "visudo -c after retire"
ok "legacy pg helper retired, visudo -c ran"
[ -e "$D1/usr/local/libexec/k2-mail-helper" ] && [ -e "$D1/etc/sudoers.d/k2-mail-helper" ] || fail "mail legacy must stay one train"
ok "legacy mail helper kept one train"
printf '%s' "$out" | grep -q "minisign + sha256 verified" || fail "provenance: $out"
[ ! -e "$D1/usr/local/bin/stalwart" ] || fail "must never touch stalwart"
ok "summary + no stalwart"

echo "== re-run: unchanged =="
run "$D1" --version 0.45.5
[ "$rc" -eq 0 ] || fail "re-run: $out"
printf '%s' "$out" | grep -q "k2-root-helper  unchanged" || fail "helper not unchanged: $out"
printf '%s' "$out" | grep -q "daemon.toml  unchanged" || fail "toml not unchanged: $out"
printf '%s' "$out" | grep -q "k2-root-helper  unchanged  (0440" || fail "sudoers not unchanged: $out"
ok "idempotent"

echo "== upgrade, then downgrade refused unless --allow-downgrade =="
run "$D1" --version 0.45.6
[ "$rc" -eq 0 ] || fail "upgrade: $out"
printf '%s' "$out" | grep -q "installed v0.45.6 (was v0.45.5)" || fail "upgrade summary: $out"
run "$D1" --version 0.45.5
expect_fail_with "downgrade" "is older"
cmp -s "$WORK/rel/v0.45.6/k2-root-helper-linux-x86_64" "$D1/usr/local/libexec/k2-root-helper" || fail "downgrade changed the helper"
run "$D1" --version 0.45.5 --allow-downgrade
[ "$rc" -eq 0 ] || fail "allow-downgrade: $out"
ok "anti-downgrade (RH8) with an explicit override"

echo "== no keys (ceremony pending): every download refused =="
D2="$WORK/box2"
NOKEYS=1 run "$D2" --version 0.45.5
expect_fail_with "no keys" "key ceremony pending"
nothing_installed "$D2" "no keys"
ok "an installer without keys refuses downloads"

echo "== signature / comment / marker refusals =="
A=k2-root-helper-linux-x86_64
cp "$WORK/rel/v0.45.5/$A.sig" "$WORK/good.sig"
printf 'untrusted comment: t\nBAD\ntrusted comment: file:%s version:0.45.5 protocol:4\nG\n' "$A" >"$WORK/rel/v0.45.5/$A.sig"
run "$D2" --version 0.45.5
expect_fail_with "bad sig" "MINISIGN VERIFICATION FAILED"
nothing_installed "$D2" "bad sig"
SHIM_GOOD_KEY=RWOTHERKEY run "$D2" --version 0.45.5
expect_fail_with "untrusted key" "MINISIGN VERIFICATION FAILED"
cp "$WORK/good.sig" "$WORK/rel/v0.45.5/$A.sig"
ok "bad signature / untrusted key refused"

release 0.45.7 4 "file:k2-daemon-linux-x86_64 version:0.45.7 protocol:4"
run "$D2" --version 0.45.7
expect_fail_with "daemon asset" "not for k2-root-helper-linux-x86_64"
release 0.45.7 4 "timestamp:1791420765	file:k2-root-helper-linux-x86_64"
run "$D2" --version 0.45.7
expect_fail_with "tauri comment" "not for k2-root-helper-linux-x86_64"
release 0.45.7 4 "file:k2-root-helper-linux-x86_64 version:0.45.8 protocol:4"
run "$D2" --version 0.45.7
expect_fail_with "comment version" "not v0.45.7"
release 0.45.7 4 "" "" 0.45.9
run "$D2" --version 0.45.7
expect_fail_with "marker mismatch" "does not match the signed comment"
release 0.45.7 4 "" "$MARK v=0.45.7 proto=4 arch=x86_64;"
run "$D2" --version 0.45.7
expect_fail_with "two markers" "exactly one"
release 0.45.7 2
run "$D2" --version 0.45.7
expect_fail_with "protocol 2" "protocol 2"
nothing_installed "$D2" "comment/marker refusals"
ok "wrong file / tauri comment / version / marker / two markers / old protocol refused"

echo "== sha256 mismatch, missing release, visudo rejection =="
release 0.45.7 4
echo "0000000000000000000000000000000000000000000000000000000000000000  $A" >"$WORK/rel/v0.45.7/$A.sha256"
run "$D2" --version 0.45.7
expect_fail_with "sha" "SHA256 MISMATCH"
run "$D2" --version 0.44.0
expect_fail_with "missing release" "ships from v0.45.5"
VISUDO_FAIL=1 run "$D2" --version 0.45.5
expect_fail_with "visudo" "visudo rejected"
nothing_installed "$D2" "sha/missing/visudo"
ok "sha mismatch, missing release and visudo rejection leave nothing"

echo "== --file: unsigned warns; a .sig beside it is checked =="
D3="$WORK/box3"
fake_elf "$WORK/k2-root-helper" 0.45.5 4 x86_64
run "$D3" --file "$WORK/k2-root-helper" --user k2std --unit k2-daemon-std --scope user
[ "$rc" -eq 0 ] || fail "--file: $out"
printf '%s' "$out" | grep -q "NOT signature-checked" || fail "--file provenance: $out"
grep -q '^user = "k2std"$' "$D3/etc/k2-root-helper/daemon.toml" || fail "toml user"
grep -q '^unit = "k2-daemon-std.service"$' "$D3/etc/k2-root-helper/daemon.toml" || fail "unit suffix"
grep -q '^scope = "user"$' "$D3/etc/k2-root-helper/daemon.toml" || fail "toml scope"
grep -q '^k2std ALL=(root) NOPASSWD: /usr/local/libexec/k2-root-helper$' "$D3/etc/sudoers.d/k2-root-helper" || fail "sudoers user"
ok "--file + --user/--unit/--scope (user-scope self-hosted shape)"
printf 'untrusted comment: t\nBAD\n' >"$WORK/k2-root-helper.sig"
D4="$WORK/box4"
run "$D4" --file "$WORK/k2-root-helper"
expect_fail_with "--file bad sig" "MINISIGN VERIFICATION FAILED"
nothing_installed "$D4" "--file bad sig"
rm -f "$WORK/k2-root-helper.sig"
printf 'not an elf' >"$WORK/notelf"
run "$D4" --file "$WORK/notelf"
expect_fail_with "non-ELF" "not an ELF"
fake_elf "$WORK/arm" 0.45.5 4 aarch64
run "$D4" --file "$WORK/arm"
expect_fail_with "wrong arch" "built for aarch64"
nothing_installed "$D4" "--file refusals"
ok "--file .sig / non-ELF / wrong-arch refusals"

echo "== symlink at the helper path; door check failing keeps legacy =="
D5="$WORK/box5"
legacy "$D5"
ln -s /bin/sh "$D5/usr/local/libexec/k2-root-helper"
run "$D5" --version 0.45.5
expect_fail_with "symlink" "is a symlink"
rm "$D5/usr/local/libexec/k2-root-helper"
K2_TEST_DOOR_OK=0 run "$D5" --version 0.45.5
[ "$rc" -eq 0 ] || fail "door-not-ok install: $out"
[ -e "$D5/usr/local/libexec/k2-pg-helper" ] || fail "pg legacy must stay when the door check fails"
ok "a planted symlink is refused; legacy stays until the new door answers"
run "$D5" --version 0.45.5 --retire-legacy-mail
[ ! -e "$D5/usr/local/libexec/k2-mail-helper" ] && [ ! -e "$D5/etc/sudoers.d/k2-mail-helper" ] || fail "--retire-legacy-mail"
[ ! -e "$D5/usr/local/libexec/k2-pg-helper" ] || fail "pg retired with the door ok"
ok "--retire-legacy-mail removes the old mail helper"

echo "== argument and environment guards =="
D6="$WORK/box6"
run "$D6" --version 0.45.5 --file "$WORK/k2-root-helper"
expect_fail_with "both" "not both"
run "$D6" --version 0.45.5 --user 'k2 ALL=(ALL) ALL'
expect_fail_with "user injection" "plain user name"
run "$D6" --version 0.45.5 --unit 'x.service
k2 ALL=(ALL) ALL'
expect_fail_with "unit injection" "systemd unit name"
run "$D6" --version 0.45.5 --scope global
expect_fail_with "scope" "system or user"
for bad in 0.45 0.45.5.1 00.45.5 0.45.5-rc1 '0.45.5;id' ' 0.45.5' 1234567.0.0; do
	run "$D6" --version "$bad"
	expect_fail_with "version '$bad'" "must look like"
done
run "$D6"
expect_fail_with "no version" "pass --version"
if grep -Ev '^[[:space:]]*#' "$INSTALLER" | grep -Eq 'detect_version|\.local/bin/k2-daemon|daemon\.port|/boot-status'; then
	fail "installer derives the version from daemon-user files"
fi
SHIM_UID=1000 run "$D6" --version 0.45.5
expect_fail_with "non-root" "run as root"
SHIM_OS=Darwin run "$D6" --version 0.45.5
expect_fail_with "non-Linux" "Linux-only"
SHIM_ARCH=aarch64 run "$D6" --version 0.45.5
expect_fail_with "aarch64 asset" "k2-root-helper-linux-aarch64"
nothing_installed "$D6" "guards"
ok "argument, version, root, OS and arch guards"

echo "== install-mail-helper.sh is a wrapper for the root installer =="
D7="$WORK/box7"
SCRIPT="$WRAPPER" run "$D7" --version 0.45.5
[ "$rc" -eq 0 ] || fail "wrapper: $out"
[ -f "$D7/usr/local/libexec/k2-root-helper" ] || fail "wrapper did not install the root helper"
[ ! -e "$D7/usr/local/libexec/k2-mail-helper" ] || fail "wrapper must not install the old helper"
ok "old name installs k2-root-helper"

echo ""
echo "PASS: install-root-helper ($pass checks)"
