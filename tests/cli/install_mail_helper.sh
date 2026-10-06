#!/usr/bin/env bash
# Hermetic test for scripts/install-mail-helper.sh. No root, no network, no
# real /usr/local or /etc: K2_INSTALL_ROOT redirects both destinations into a
# temp dir; PATH shims stand in for id/uname/install/stat/visudo/sudo/minisign
# (minisign "verifies" when the .sig carries GOOD-SIG). Release assets are a
# file:// mirror. Run with: bash tests/cli/install_mail_helper.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
INSTALLER="$ROOT/scripts/install-mail-helper.sh"
[ -f "$INSTALLER" ] || { echo "FAIL: missing $INSTALLER" >&2; exit 1; }

pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

WORK="$(mktemp -d -t k2-install-mail-helper-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
mkdir -p "$SHIMS"
REAL_INSTALL="$(command -v install)"
REAL_STAT="$(command -v stat)"
# Octal mode, GNU (Linux) or BSD (macOS) stat.
mode_of() { "$REAL_STAT" -c '%a' "$1" 2>/dev/null || "$REAL_STAT" -f '%Lp' "$1"; }

cat >"$SHIMS/id" <<'SH'
#!/bin/sh
# `id -u` -> SHIM_UID (default 0); `id <user>` -> exists.
if [ "${1:-}" = "-u" ]; then echo "${SHIM_UID:-0}"; exit 0; fi
exit 0
SH
cat >"$SHIMS/uname" <<'SH'
#!/bin/sh
case "${1:-}" in -s) echo "${SHIM_OS:-Linux}" ;; -m) echo "${SHIM_ARCH:-x86_64}" ;; *) echo Linux ;; esac
SH
cat >"$SHIMS/install" <<SH
#!/bin/sh
# Drop -o/-g (non-root test run); record the owner the script asked for.
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
# GNU \`stat -c '%U:%G %a' f\` -> root:root <octal mode> (owner is the test user).
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
cat >"$SHIMS/minisign" <<'SH'
#!/bin/sh
# minisign -Vm <file> -p <pub> -x <sig>
sig=""
while [ $# -gt 0 ]; do
    case "$1" in -x) sig="$2"; shift 2 ;; *) shift ;; esac
done
grep -q GOOD-SIG "$sig"
SH
chmod +x "$SHIMS"/*
export PATH="$SHIMS:$PATH"

# Release mirror: v0.44.1 assets for x86_64.
REL="$WORK/rel/v0.44.1"
mkdir -p "$REL"
ASSET="k2-mail-helper-linux-x86_64"
printf '\177ELF-fake-k2-mail-helper-0.44.1' >"$REL/$ASSET"
printf 'untrusted comment: test\nGOOD-SIG\n' >"$REL/$ASSET.sig"
(cd "$REL" && sha256sum "$ASSET" >"$ASSET.sha256")

run() {
    # run <dest-root> [args...]; sets out/rc
    local dest="$1"
    shift
    set +e
    out="$(K2_INSTALL_ROOT="$dest" K2_RELEASE_BASE="file://$WORK/rel" \
        bash "$INSTALLER" "$@" 2>&1)"
    rc=$?
    set -e
}

assert_nothing_installed() {
    local dest="$1" label="$2"
    [ ! -e "$dest/usr/local/libexec/k2-mail-helper" ] || fail "$label: helper must not be installed"
    [ ! -e "$dest/etc/sudoers.d/k2-mail-helper" ] || fail "$label: sudoers must not be written"
    local f
    for f in "$dest"/usr/local/libexec/.k2-mail-helper.new.* "$dest"/etc/sudoers.d/.k2-mail-helper.new.*; do
        [ ! -e "$f" ] || fail "$label: staged file left behind: $f"
    done
}

expect_fail_with() {
    # expect_fail_with <label> <needle>  (uses rc/out from run)
    if [ "$rc" -eq 0 ]; then
        fail "$1: expected failure, got exit 0: $out"
    fi
    if ! printf '%s' "$out" | grep -q -- "$2"; then
        fail "$1: missing '$2' in: $out"
    fi
}

echo "== fresh install from a verified release asset =="
D1="$WORK/box1"
mkdir -p "$D1/etc/sudoers.d"
printf 'k2 ALL=(root) NOPASSWD: /usr/local/libexec/k2-pg-helper\n' >"$D1/etc/sudoers.d/k2-pg-helper"
cp "$D1/etc/sudoers.d/k2-pg-helper" "$WORK/pg-before"
run "$D1" --version 0.44.1
[ "$rc" -eq 0 ] || fail "fresh install exit $rc: $out"
printf '%s\n' "$out" | sed 's/^/    | /'
cmp -s "$REL/$ASSET" "$D1/usr/local/libexec/k2-mail-helper" || fail "helper bytes differ"
ok "helper bytes match the release asset"
[ "$(mode_of "$D1/usr/local/libexec/k2-mail-helper")" = "755" ] || fail "helper mode"
ok "helper mode 0755"
grep -q -- "-o root" "$WORK/install-owner.log" || fail "install must ask for -o root"
grep -q -- "-g root" "$WORK/install-owner.log" || fail "install must ask for -g root"
ok "installed as root:root"
expected="$(printf 'k2 ALL=(root) NOPASSWD: /usr/local/libexec/k2-mail-helper\nDefaults:k2 !requiretty')"
[ "$(cat "$D1/etc/sudoers.d/k2-mail-helper")" = "$expected" ] \
    || fail "sudoers content: $(cat "$D1/etc/sudoers.d/k2-mail-helper")"
ok "sudoers is exactly the two lines"
[ "$(mode_of "$D1/etc/sudoers.d/k2-mail-helper")" = "440" ] || fail "sudoers mode"
ok "sudoers mode 0440"
grep -q -- "-cf" "$WORK/visudo.log" || fail "visudo -cf never ran"
ok "visudo -cf validated"
cmp -s "$WORK/pg-before" "$D1/etc/sudoers.d/k2-pg-helper" || fail "k2-pg-helper sudoers changed"
ok "k2-pg-helper sudoers untouched"
printf '%s' "$out" | grep -q "helper  : /usr/local/libexec/k2-mail-helper  installed" || fail "summary: $out"
printf '%s' "$out" | grep -q "sudoers : /etc/sudoers.d/k2-mail-helper  written" || fail "summary: $out"
printf '%s' "$out" | grep -q "minisign + sha256 verified" || fail "summary provenance: $out"
ok "summary names installed / written / verified"
[ ! -e "$D1/usr/local/bin/stalwart" ] || fail "must never create stalwart"
ok "no /usr/local/bin/stalwart"

echo "== re-run, same version: no-op apart from re-verifying =="
run "$D1" --version v0.44.1
[ "$rc" -eq 0 ] || fail "re-run exit $rc: $out"
printf '%s' "$out" | grep -q "k2-mail-helper  unchanged" || fail "re-run helper not unchanged: $out"
printf '%s' "$out" | grep -q "sudoers.d/k2-mail-helper  unchanged" || fail "re-run sudoers not unchanged: $out"
printf '%s' "$out" | grep -q "verified" || fail "re-run must still verify: $out"
ok "second run reports unchanged / unchanged"

echo "== bad signature: refuse, nothing installed =="
D2="$WORK/box2"
printf 'untrusted comment: test\nBAD\n' >"$REL/$ASSET.sig"
run "$D2" --version 0.44.1
[ "$rc" -ne 0 ] || fail "bad sig must fail: $out"
printf '%s' "$out" | grep -q "MINISIGN VERIFICATION FAILED" || fail "bad sig message: $out"
assert_nothing_installed "$D2" "bad sig"
ok "bad signature refused"
printf 'untrusted comment: test\nGOOD-SIG\n' >"$REL/$ASSET.sig"

echo "== sha256 mismatch: refuse =="
cp "$REL/$ASSET.sha256" "$WORK/sha.bak"
echo "0000000000000000000000000000000000000000000000000000000000000000  $ASSET" >"$REL/$ASSET.sha256"
run "$D2" --version 0.44.1
[ "$rc" -ne 0 ] || fail "sha mismatch must fail: $out"
printf '%s' "$out" | grep -q "SHA256 MISMATCH" || fail "sha message: $out"
assert_nothing_installed "$D2" "sha mismatch"
ok "sha256 mismatch refused"
cp "$WORK/sha.bak" "$REL/$ASSET.sha256"

echo "== missing release (pre-0.44.1): clear message =="
run "$D2" --version 0.44.0
[ "$rc" -ne 0 ] || fail "missing release must fail"
printf '%s' "$out" | grep -q "exist from v0.44.1" || fail "missing release message: $out"
assert_nothing_installed "$D2" "missing release"
ok "missing release explains"

echo "== visudo rejects: nothing left behind =="
VISUDO_FAIL=1 run "$D2" --version 0.44.1
[ "$rc" -ne 0 ] || fail "visudo failure must fail: $out"
printf '%s' "$out" | grep -q "visudo rejected" || fail "visudo message: $out"
assert_nothing_installed "$D2" "visudo failure"
ok "visudo rejection leaves nothing"

echo "== --file: local binary, root:root 0755 =="
D3="$WORK/box3"
printf '\177ELF-local-build' >"$WORK/k2-mail-helper"
run "$D3" --file "$WORK/k2-mail-helper"
[ "$rc" -eq 0 ] || fail "--file exit $rc: $out"
cmp -s "$WORK/k2-mail-helper" "$D3/usr/local/libexec/k2-mail-helper" || fail "--file bytes"
printf '%s' "$out" | grep -q "local file" || fail "--file provenance: $out"
ok "--file installs the local binary"
printf 'not an elf' >"$WORK/notelf"
D4="$WORK/box4"
run "$D4" --file "$WORK/notelf"
[ "$rc" -ne 0 ] || fail "non-ELF must fail"
assert_nothing_installed "$D4" "non-ELF"
ok "non-ELF --file refused"

echo "== argument and environment guards =="
run "$D4" --version 0.44.1 --file "$WORK/k2-mail-helper"
expect_fail_with "--version + --file" "not both"
ok "--version with --file refused"
run "$D4" --version 0.44.1 --user 'k2 ALL=(ALL) ALL'
expect_fail_with "--user injection" "plain user name"
ok "sudoers user injection refused"
run "$D4" --version 'v1;rm -rf /'
expect_fail_with "bad version" "must look like"
ok "malformed version refused"
SHIM_UID=1000 run "$D4" --version 0.44.1
expect_fail_with "non-root" "run as root"
ok "non-root refused"
SHIM_OS=Darwin run "$D4" --version 0.44.1
expect_fail_with "non-Linux" "Linux-only"
ok "non-Linux refused"
SHIM_ARCH=aarch64 run "$D4" --version 0.44.1
expect_fail_with "aarch64 asset name" "k2-mail-helper-linux-aarch64"
ok "aarch64 asks for the aarch64 asset"
assert_nothing_installed "$D4" "guards"

echo ""
echo "PASS: install-mail-helper ($pass checks)"
