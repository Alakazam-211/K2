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

echo "== fresh install from a verified release asset; legacy helpers retired =="
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
[ ! -e "$D1/usr/local/libexec/k2-mail-helper" ] && [ ! -e "$D1/etc/sudoers.d/k2-mail-helper" ] || fail "mail legacy not retired"
ok "legacy mail helper retired at install (Rosson 2026-10-10)"
BK="$(ls -d "$D1"/var/backups/k2-root-helper/*/ | head -n1)"
[ -n "$BK" ] || fail "no backup dir"
for f in k2-pg-helper k2-mail-helper; do
	[ -f "$BK/$f" ] || fail "backup lacks $f (sudoers or binary)"
done
[ "$(cat "$BK/k2-mail-helper" | head -c 4 | od -An -c | tr -d ' \n')" = '177ELF' ] || [ -n "$(cat "$BK/k2-mail-helper")" ] || fail "backup content"
[ "$(mode_of "$BK")" = "700" ] || fail "backup dir mode $(mode_of "$BK")"
printf '%s' "$out" | grep -q "backups     : /var/backups/k2-root-helper/" || fail "summary names the backups: $out"
printf '%s' "$out" | grep -q "mail-sidecar: none" || fail "sidecar none: $out"
ok "every removed file backed up first (0700 dir), summary names it"
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

echo "== the shipped installer carries the release keys, not the test key =="
grep -qx 'K2_ROOT_HELPER_PUBKEYS=(RWSV9wBCWGnbGBvUcoZDJWucBgmvxNmRbApWopWr+kUOYbmuZ7+kSYI1 RWR6KVLQytLC6Yiiy/nCzF6HFJXp5OwkF+AjtMhlznwZfucjf0DoDY5W)' "$INSTALLER" \
	|| fail "installer must carry the 2026-10-10 primary + backup keys"
D2="$WORK/box2"
NOKEYS=1 run "$D2" --version 0.45.5
expect_fail_with "release keys only" "MINISIGN VERIFICATION FAILED"
nothing_installed "$D2" "release keys only"
ok "with only the release keys a test-signed download is refused"

echo "== no keys: every download refused (fail closed) =="
sed 's/^K2_ROOT_HELPER_PUBKEYS=(.*)$/K2_ROOT_HELPER_PUBKEYS=()/' "$INSTALLER" >"$WORK/install-nokeys.sh"
grep -qx 'K2_ROOT_HELPER_PUBKEYS=()' "$WORK/install-nokeys.sh" || fail "could not strip the keys"
NOKEYS=1 SCRIPT="$WORK/install-nokeys.sh" run "$D2" --version 0.45.5
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
run "$D2" --version 0.45.9
expect_fail_with "missing release" "ships from v0.45.4"
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
[ -e "$D5/usr/local/libexec/k2-mail-helper" ] || fail "mail legacy must stay when the door check fails"
ok "a planted symlink is refused; legacy stays until the new door answers"
run "$D5" --version 0.45.5 --retire-legacy-mail
[ "$rc" -eq 0 ] || fail "--retire-legacy-mail must still be accepted: $out"
[ ! -e "$D5/usr/local/libexec/k2-mail-helper" ] && [ ! -e "$D5/etc/sudoers.d/k2-mail-helper" ] || fail "mail retired"
[ ! -e "$D5/usr/local/libexec/k2-pg-helper" ] || fail "pg retired with the door ok"
ok "the old --retire-legacy-mail flag is accepted (retirement is the default)"

echo "== k2-mail-sidecar sudoers (akzm, lztek): retired only when K2 wrote it =="
D8="$WORK/box8"
mkdir -p "$D8/etc/sudoers.d"
printf '# K2 hosted mail\nk2 ALL=(root) NOPASSWD: /usr/bin/systemctl restart stalwart, /bin/systemctl start stalwart.service, /usr/local/libexec/k2-mail-sidecar\nDefaults:k2 !requiretty\n' \
	>"$D8/etc/sudoers.d/k2-mail-sidecar"
cp "$D8/etc/sudoers.d/k2-mail-sidecar" "$WORK/sidecar.orig"
run "$D8" --version 0.45.5
[ "$rc" -eq 0 ] || fail "sidecar install: $out"
[ ! -e "$D8/etc/sudoers.d/k2-mail-sidecar" ] || fail "a K2-shaped k2-mail-sidecar must be retired"
cmp -s "$WORK/sidecar.orig" "$(ls -d "$D8"/var/backups/k2-root-helper/*/ | head -n1)/k2-mail-sidecar" || fail "sidecar backup bytes"
printf '%s' "$out" | grep -q "mail-sidecar: retired: /etc/sudoers.d/k2-mail-sidecar" || fail "sidecar summary: $out"
ok "a K2-written k2-mail-sidecar is backed up and retired"

# The exact line the 2026-09 mail-sidecar provisioning wrote on akzm and lztek.
legacy "$D8"
printf 'k2 ALL=(root) NOPASSWD: /usr/bin/systemctl, /usr/bin/chown, /usr/sbin/useradd\n' >"$D8/etc/sudoers.d/k2-mail-sidecar"
run "$D8" --version 0.45.5
[ "$rc" -eq 0 ] || fail "fleet sidecar install: $out"
[ ! -e "$D8/etc/sudoers.d/k2-mail-sidecar" ] || fail "the 2026-09 fleet k2-mail-sidecar line must be retired"
printf '%s' "$out" | grep -q "mail-sidecar: retired: /etc/sudoers.d/k2-mail-sidecar" || fail "fleet sidecar summary: $out"
ok "the 2026-09 fleet k2-mail-sidecar line (akzm, lztek) is retired"
for body in 'k2 ALL=(ALL) NOPASSWD: ALL' 'k2 ALL=(root) NOPASSWD: /usr/bin/systemctl restart stalwart, /bin/bash' 'admin ALL=(root) NOPASSWD: /usr/bin/systemctl restart stalwart' '%sudo ALL=(ALL) ALL'; do
	printf '%s\n' "$body" >"$D8/etc/sudoers.d/k2-mail-sidecar"
	run "$D8" --version 0.45.5
	[ "$rc" -eq 0 ] || fail "foreign sidecar must not fail the install: $out"
	[ "$(cat "$D8/etc/sudoers.d/k2-mail-sidecar")" = "$body" ] || fail "foreign sidecar changed: $body"
	printf '%s' "$out" | grep -q "LEFT IN PLACE: /etc/sudoers.d/k2-mail-sidecar is not one K2 wrote" || fail "foreign sidecar report ($body): $out"
done
ok "any other k2-mail-sidecar content is reported and left alone"

echo "== the sidecar's /usr/local/bin PATH shims go with the line, or nothing goes =="
FLEET_LINE='k2 ALL=(root) NOPASSWD: /usr/bin/systemctl, /usr/bin/chown, /usr/sbin/useradd'
SHIM_NAMES="k2-if-k2-sudo systemctl chown useradd"
plant_shims() {
	# plant_shims <root>: the four files exactly as akzm/lztek have them.
	local b="$1/usr/local/bin"
	mkdir -p "$b"
	printf '#!/bin/sh\nREAL="$1"; shift\nif [ "$(id -un)" = k2 ]; then\n  exec sudo -n "$REAL" "$@"\nfi\nexec "$REAL" "$@"\n' >"$b/k2-if-k2-sudo"
	printf '#!/bin/sh\nexec /usr/local/bin/k2-if-k2-sudo /usr/bin/systemctl "$@"\n' >"$b/systemctl"
	printf '#!/bin/sh\nexec /usr/local/bin/k2-if-k2-sudo /usr/bin/chown "$@"\n' >"$b/chown"
	printf '#!/bin/sh\nexec /usr/local/bin/k2-if-k2-sudo /usr/sbin/useradd "$@"\n' >"$b/useradd"
	chmod 0755 "$b"/*
}
D12="$WORK/box12"
mkdir -p "$D12/etc/sudoers.d"
plant_shims "$D12"
[ "$(wc -c <"$D12/usr/local/bin/k2-if-k2-sudo" | tr -d ' ')" = 106 ] || fail "fixture: k2-if-k2-sudo must be the 106-byte fleet file"
mkdir -p "$WORK/shims.orig"
cp -p "$D12"/usr/local/bin/* "$WORK/shims.orig/"
printf '%s\n' "$FLEET_LINE" >"$D12/etc/sudoers.d/k2-mail-sidecar"
run "$D12" --version 0.45.5
[ "$rc" -eq 0 ] || fail "line + shims: $out"
[ ! -e "$D12/etc/sudoers.d/k2-mail-sidecar" ] || fail "line + shims: the line must be retired"
BK12="$(ls -d "$D12"/var/backups/k2-root-helper/*/ | head -n1)"
for n in $SHIM_NAMES; do
	[ ! -e "$D12/usr/local/bin/$n" ] || fail "line + shims: /usr/local/bin/$n must be retired"
	cmp -s "$WORK/shims.orig/$n" "$BK12/$n" || fail "line + shims: backup bytes of $n"
done
printf '%s' "$out" | grep -q "mail-sidecar: retired: /etc/sudoers.d/k2-mail-sidecar" || fail "line + shims summary (line): $out"
printf '%s' "$out" | grep -q "path-shims  : retired: /usr/local/bin/k2-if-k2-sudo /usr/local/bin/systemctl /usr/local/bin/chown /usr/local/bin/useradd" || fail "line + shims summary (shims): $out"
ok "line + the four known shims: all retired and backed up"

# Whitespace and comment lines don't matter; the exec line does.
D13="$WORK/box13"
mkdir -p "$D13/etc/sudoers.d"
plant_shims "$D13"
printf '#!/bin/sh\n# K2 mail sidecar shim\n\n   exec   /usr/local/bin/k2-if-k2-sudo  /usr/bin/chown "$@"  \n' >"$D13/usr/local/bin/chown"
printf '%s\n' "$FLEET_LINE" >"$D13/etc/sudoers.d/k2-mail-sidecar"
run "$D13" --version 0.45.5
[ "$rc" -eq 0 ] || fail "spaced shim: $out"
[ ! -e "$D13/usr/local/bin/chown" ] && [ ! -e "$D13/etc/sudoers.d/k2-mail-sidecar" ] || fail "a commented/spaced wrapper still matches: $out"
ok "comments and whitespace in a wrapper still match"

check_left_in_place() {
	# check_left_in_place <label> <root> <foreign shim>: line + every shim untouched.
	local n
	[ "$rc" -eq 0 ] || fail "$1: must not fail the install: $out"
	[ "$(cat "$2/etc/sudoers.d/k2-mail-sidecar")" = "$FLEET_LINE" ] || fail "$1: the line must stay"
	for n in $SHIM_NAMES; do [ -e "$2/usr/local/bin/$n" ] || [ -L "$2/usr/local/bin/$n" ] || fail "$1: /usr/local/bin/$n must stay"; done
	printf '%s' "$out" | grep -q "mail-sidecar: LEFT IN PLACE: /etc/sudoers.d/k2-mail-sidecar — PATH shims" || fail "$1: line report: $out"
	printf '%s' "$out" | grep -q "path-shims  : LEFT IN PLACE:.*not what K2 wrote: $3" || fail "$1: shim report: $out"
}
i=0
for mod in \
	'k2-if-k2-sudo|#!/bin/sh\nREAL="$1"; shift\nif [ "$(id -un)" = k2 ]; then\n  exec sudo "$REAL" "$@"\nfi\nexec "$REAL" "$@"\n' \
	'systemctl|#!/bin/sh\nexec /usr/local/bin/k2-if-k2-sudo /usr/bin/systemctl "$@"\nrm -rf /tmp/x\n' \
	'useradd|#!/bin/sh\nexec /usr/local/bin/k2-if-k2-sudo /usr/bin/useradd "$@"\n' \
	'chown|exec /usr/local/bin/k2-if-k2-sudo /usr/bin/chown "$@"\n' \
	'systemctl|#!/bin/sh\nexec /usr/local/bin/k2-if-k2-sudo /usr/bin/systemctl $@\n'; do
	i=$((i + 1))
	D="$WORK/box14-$i"
	mkdir -p "$D/etc/sudoers.d"
	plant_shims "$D"
	printf "${mod#*|}" >"$D/usr/local/bin/${mod%%|*}"
	printf '%s\n' "$FLEET_LINE" >"$D/etc/sudoers.d/k2-mail-sidecar"
	run "$D" --version 0.45.5
	check_left_in_place "modified ${mod%%|*} #$i" "$D" "/usr/local/bin/${mod%%|*}"
	[ ! -e "$D/var/backups/k2-root-helper" ] || fail "modified #$i: nothing may be backed up or removed"
done
D="$WORK/box14-link"
mkdir -p "$D/etc/sudoers.d"
plant_shims "$D"
rm "$D/usr/local/bin/useradd"
ln -s /usr/sbin/useradd "$D/usr/local/bin/useradd"
printf '%s\n' "$FLEET_LINE" >"$D/etc/sudoers.d/k2-mail-sidecar"
run "$D" --version 0.45.5
check_left_in_place "symlinked useradd" "$D" "/usr/local/bin/useradd"
ok "line + one modified (or symlinked) shim: nothing retired, both reported"

D15="$WORK/box15"
mkdir -p "$D15/etc/sudoers.d"
plant_shims "$D15"
run "$D15" --version 0.45.5
[ "$rc" -eq 0 ] || fail "shims without the line: $out"
for n in $SHIM_NAMES; do cmp -s "$WORK/shims.orig/$n" "$D15/usr/local/bin/$n" || fail "shims without the line: $n must stay as is"; done
printf '%s' "$out" | grep -q "mail-sidecar: none" || fail "shims without the line (line): $out"
printf '%s' "$out" | grep -q "path-shims  : LEFT IN PLACE: /usr/local/bin/k2-if-k2-sudo /usr/local/bin/systemctl /usr/local/bin/chown /usr/local/bin/useradd (present without /etc/sudoers.d/k2-mail-sidecar)" || fail "shims without the line (report): $out"
ok "shims without the line: left alone and reported"

D16="$WORK/box16"
mkdir -p "$D16/etc/sudoers.d"
printf '%s\n' "$FLEET_LINE" >"$D16/etc/sudoers.d/k2-mail-sidecar"
plant_shims "$D16"
K2_TEST_DOOR_OK=0 run "$D16" --version 0.45.5
[ "$rc" -eq 0 ] || fail "door not ok + shims: $out"
for n in $SHIM_NAMES; do [ -e "$D16/usr/local/bin/$n" ] || fail "door not ok: $n must stay"; done
[ -e "$D16/etc/sudoers.d/k2-mail-sidecar" ] || fail "door not ok: line must stay"
printf '%s' "$out" | grep -q "path-shims  : kept (the root helper did not answer" || fail "door not ok report: $out"
run "$D16" --version 0.45.5
printf '%s' "$out" | grep -q "path-shims  : retired: /usr/local/bin/k2-if-k2-sudo" || fail "door ok on rerun: $out"
[ ! -e "$D16/etc/sudoers.d/k2-mail-sidecar" ] || fail "door ok on rerun: line retired"
run "$D16" --version 0.45.5
printf '%s' "$out" | grep -q "path-shims  : none" || fail "idempotent rerun: $out"
ok "shims stay while the root helper doesn't answer; a rerun retires them; a third run sees none"

echo "== --user other than k2: the k2-only shims never match =="
D17="$WORK/box17"
mkdir -p "$D17/etc/sudoers.d"
plant_shims "$D17"
printf 'bob ALL=(root) NOPASSWD: /usr/bin/systemctl, /usr/bin/chown, /usr/sbin/useradd\n' >"$D17/etc/sudoers.d/k2-mail-sidecar"
run "$D17" --version 0.45.5 --user bob
[ "$rc" -eq 0 ] || fail "--user bob + shims: $out"
[ -e "$D17/etc/sudoers.d/k2-mail-sidecar" ] || fail "--user bob: the line must stay while unknown shims are present"
for n in $SHIM_NAMES; do [ -e "$D17/usr/local/bin/$n" ] || fail "--user bob: $n must stay"; done
printf '%s' "$out" | grep -q "mail-sidecar: LEFT IN PLACE: /etc/sudoers.d/k2-mail-sidecar — PATH shims" || fail "--user bob report: $out"
rm "$D17"/usr/local/bin/*
run "$D17" --version 0.45.5 --user bob
[ ! -e "$D17/etc/sudoers.d/k2-mail-sidecar" ] || fail "--user bob without shims: the line retires as before"
printf '%s' "$out" | grep -q "path-shims  : retired: (none)" || fail "--user bob without shims report: $out"
ok "--user bob: k2-form shims count as unknown and hold the line; without shims the line retires"

echo "== daemon older than 0.45.4: refuse, change nothing (iascm 0.41.3) =="
D9="$WORK/box9"
legacy "$D9"
run "$D9" --version 0.41.3
expect_fail_with "old --version" "older than v0.45.4"
K2_TEST_RUNNING_DAEMON_VERSION=0.41.3 run "$D9" --version 0.45.5
expect_fail_with "old running daemon" "is v0.41.3, older than v0.45.4"
nothing_installed "$D9" "old daemon"
[ -e "$D9/usr/local/libexec/k2-mail-helper" ] && [ -e "$D9/usr/local/libexec/k2-pg-helper" ] || fail "old daemon: legacy must stay"
K2_TEST_RUNNING_DAEMON_VERSION=0.45.4 run "$D9" --version 0.45.5
[ "$rc" -eq 0 ] || fail "0.45.4 running daemon: $out"
printf '%s' "$out" | grep -q "daemon      : v0.45.4 (running)" || fail "daemon line: $out"
ok "a daemon below 0.45.4 (asked for or running) is refused before any change"

echo "== /boot-status: the TOP-LEVEL version, not ops.version (keys sort; ops comes first) =="
D9B="$WORK/box9b"
BOOT="$WORK/boot-status.json"
# Shape of a real 0.46.2 answer: ops.version "1" precedes the top-level one,
# and a string value that merely contains "version":"x" must not count.
printf '%s' '{"airgap":{"baked":false},"detail":"say \"version\":\"9.9.9\"","features":["a"],"ops":{"supported":true,"version":"1"},"phase":"ready","protocol":1,"version":"0.41.3","webClient":{"enabled":true}}' >"$BOOT"
legacy "$D9B"
K2_TEST_BOOT_STATUS_FILE="$BOOT" run "$D9B" --version 0.45.5
expect_fail_with "old daemon via boot-status" "is v0.41.3, older than v0.45.4"
nothing_installed "$D9B" "old daemon via boot-status"
printf '%s' '{"ops":{"supported":true,"version":"1"},"phase":"ready","version":"0.46.2"}' >"$BOOT"
K2_TEST_BOOT_STATUS_FILE="$BOOT" run "$D9B" --version 0.45.5
[ "$rc" -eq 0 ] || fail "0.46.2 via boot-status: $out"
printf '%s' "$out" | grep -q "daemon      : v0.46.2 (running)" || fail "boot-status daemon line: $out"
printf '%s' "$out" | grep -q "could not read the running daemon's version" && fail "boot-status: must not warn: $out"
D9C="$WORK/box9c"
printf '%s' '{"ops":{"supported":true,"version":"1"},"phase":"ready"}' >"$BOOT"
K2_TEST_BOOT_STATUS_FILE="$BOOT" run "$D9C" --version 0.45.5
[ "$rc" -eq 0 ] || fail "no top-level version: $out"
printf '%s' "$out" | grep -q "could not read the running daemon's version" || fail "no top-level version must warn: $out"
ok "boot-status: top-level version read past a nested ops.version; none at top level warns"

echo "== a box with no prior helpers at all (iascm shape) =="
D10="$WORK/box10"
run "$D10" --version 0.45.5
[ "$rc" -eq 0 ] || fail "bare box: $out"
printf '%s' "$out" | grep -q "legacy pg   : retired: (none)" || fail "bare pg line: $out"
printf '%s' "$out" | grep -q "legacy mail : retired: (none)" || fail "bare mail line: $out"
[ ! -e "$D10/var/backups/k2-root-helper" ] || fail "no backup dir when nothing was removed"
ok "no prior helpers: installs, retires nothing, makes no backup dir"

echo "== a non-k2 daemon user (nsi: alakazamlabs) =="
D11="$WORK/box11"
mkdir -p "$D11/usr/local/libexec" "$D11/etc/sudoers.d"
printf 'old pg helper' >"$D11/usr/local/libexec/k2-pg-helper"
printf 'alakazamlabs ALL=(root) NOPASSWD: /usr/local/libexec/k2-pg-helper\n' >"$D11/etc/sudoers.d/k2-pg-helper"
run "$D11" --version 0.45.5 --user alakazamlabs --unit k2-daemon.service
[ "$rc" -eq 0 ] || fail "alakazamlabs: $out"
expected="$(printf 'alakazamlabs ALL=(root) NOPASSWD: /usr/local/libexec/k2-root-helper\nDefaults:alakazamlabs !requiretty')"
[ "$(cat "$D11/etc/sudoers.d/k2-root-helper")" = "$expected" ] || fail "sudoers for alakazamlabs: $(cat "$D11/etc/sudoers.d/k2-root-helper")"
grep -q '^user = "alakazamlabs"$' "$D11/etc/k2-root-helper/daemon.toml" || fail "daemon.toml user alakazamlabs"
[ ! -e "$D11/usr/local/libexec/k2-pg-helper" ] || fail "pg legacy retired for alakazamlabs"
ok "--user alakazamlabs: sudoers, daemon.toml and retirement follow the user"

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
# The running daemon's /boot-status may only REFUSE an install (too old);
# it never picks the version installed, and its binary never runs.
if grep -Ev '^[[:space:]]*#' "$INSTALLER" | grep -Eq 'detect_version|\.local/bin/k2-daemon'; then
	fail "installer runs or reads the daemon user's binary"
fi
if grep -Ev '^[[:space:]]*#' "$INSTALLER" | grep -Eq '(VERSION|SRC|FILE)="?\$\{?RUNNING_DAEMON'; then
	fail "installer derives the version from the running daemon"
fi
if grep -Ev '^[[:space:]]*#' "$INSTALLER" | grep -F 'daemon.port' | grep -vq -- '! -L'; then
	fail "the port file read must refuse a symlink"
fi
SHIM_UID=1000 run "$D6" --version 0.45.5
expect_fail_with "non-root" "run as root"
SHIM_OS=Darwin run "$D6" --version 0.45.5
expect_fail_with "non-Linux" "Linux-only"
SHIM_ARCH=aarch64 run "$D6" --version 0.45.5
expect_fail_with "aarch64 asset" "k2-root-helper-linux-aarch64"
nothing_installed "$D6" "guards"
ok "argument, version, root, OS and arch guards"

echo "== no minisign: refused up front with the distro's install hint =="
# A PATH with every tool this test uses EXCEPT minisign (shims first, so the
# id/uname/install/... stand-ins still win).
NOMS="$WORK/no-minisign-bin"
mkdir -p "$NOMS"
IFS=: read -r -a _dirs <<<"$PATH"
for _d in "${_dirs[@]}"; do
	[ -d "$_d" ] || continue
	for _f in "$_d"/*; do
		_b="$(basename "$_f")"
		[ "$_b" != minisign ] && [ -x "$_f" ] && [ ! -e "$NOMS/$_b" ] && ln -s "$_f" "$NOMS/$_b"
	done
done
[ ! -e "$NOMS/minisign" ] || fail "test PATH still has minisign"
os_release() { printf 'NAME="%s"\nID=%s\n%s' "$2" "$3" "${4:+ID_LIKE=$4
}" >"$1"; }
os_release "$WORK/os-debian" "Debian GNU/Linux" debian
os_release "$WORK/os-ubuntu" "Ubuntu" ubuntu debian
os_release "$WORK/os-rocky" "Rocky Linux" '"rocky"' '"rhel centos fedora"'
os_release "$WORK/os-arch" "Arch Linux" arch
os_release "$WORK/os-unknown" "Plan 9" plan9
DNM="$WORK/box-nominisign"
mkdir -p "$DNM"
for _case in "debian:apt-get install -y minisign" "ubuntu:apt-get install -y minisign" \
	"rocky:dnf install -y epel-release && dnf install -y minisign" \
	"arch:pacman -S --needed minisign" "unknown:github.com/jedisct1/minisign/releases"; do
	# A release that would 404 proves the refusal comes BEFORE any download.
	PATH="$NOMS" K2_TEST_OS_RELEASE="$WORK/os-${_case%%:*}" run "$DNM" --version 0.45.9
	expect_fail_with "no minisign (${_case%%:*})" "minisign is not installed"
	printf '%s' "$out" | grep -qF -- "${_case#*:}" || fail "no minisign (${_case%%:*}): hint '${_case#*:}' missing: $out"
	printf '%s' "$out" | grep -q "Nothing was downloaded or changed" || fail "no minisign: $out"
	if printf '%s' "$out" | grep -q "download failed\|downloading"; then fail "no minisign: a download was attempted: $out"; fi
done
nothing_installed "$DNM" "no minisign"
# Ubuntu/Debian ship /etc/os-release -> ../usr/lib/os-release (a relative
# symlink): the distro hint must follow it, not fall back to the generic one.
mkdir -p "$WORK/osl/etc" "$WORK/osl/usr/lib"
os_release "$WORK/osl/usr/lib/os-release" "Ubuntu" ubuntu debian
ln -s ../usr/lib/os-release "$WORK/osl/etc/os-release"
[ -L "$WORK/osl/etc/os-release" ] || fail "test setup: os-release must be a symlink"
PATH="$NOMS" K2_TEST_OS_RELEASE="$WORK/osl/etc/os-release" run "$DNM" --version 0.45.9
expect_fail_with "no minisign (symlinked os-release)" "minisign is not installed"
printf '%s' "$out" | grep -qF -- "apt-get install -y minisign" || fail "symlinked os-release: apt hint missing: $out"
if printf '%s' "$out" | grep -q "jedisct1/minisign/releases"; then fail "symlinked os-release: generic hint shown: $out"; fi
# A dangling link still falls back to the generic hint (never an error).
ln -s ../usr/lib/no-such-file "$WORK/osl/etc/os-release-dangling"
PATH="$NOMS" K2_TEST_OS_RELEASE="$WORK/osl/etc/os-release-dangling" run "$DNM" --version 0.45.9
expect_fail_with "no minisign (dangling os-release)" "minisign is not installed"
printf '%s' "$out" | grep -q "jedisct1/minisign/releases" || fail "dangling os-release: generic hint missing: $out"
nothing_installed "$DNM" "no minisign (symlinked os-release)"
# --file with no .sig never needs minisign (operator-supplied build).
fake_elf "$WORK/local-helper" 0.45.5 4 x86_64
PATH="$NOMS" run "$DNM" --file "$WORK/local-helper"
[ "$rc" -eq 0 ] || fail "--file without .sig must not need minisign: $out"
ok "minisign checked before the download, with apt/dnf/pacman/fallback hints"

echo "== install-mail-helper.sh is a wrapper for the root installer =="
D7="$WORK/box7"
SCRIPT="$WRAPPER" run "$D7" --version 0.45.5
[ "$rc" -eq 0 ] || fail "wrapper: $out"
[ -f "$D7/usr/local/libexec/k2-root-helper" ] || fail "wrapper did not install the root helper"
[ ! -e "$D7/usr/local/libexec/k2-mail-helper" ] || fail "wrapper must not install the old helper"
ok "old name installs k2-root-helper"

echo "== --creds-unit appends to the systemd-cred allow list (prd-vault-gaps-v1 item 4) =="
D8="$WORK/box8"
run "$D8" --version 0.45.5 --creds-unit k2-watch.service --creds-unit k2-control-plane.service
[ "$rc" -eq 0 ] || fail "--creds-unit: $out"
ALLOWF="$D8/etc/k2-root-helper/systemd-cred-units"
[ "$(cat "$ALLOWF")" = "$(printf 'k2-watch.service\nk2-control-plane.service')" ] || fail "allow list: $(cat "$ALLOWF")"
[ "$(mode_of "$ALLOWF")" = 644 ] || fail "allow list mode $(mode_of "$ALLOWF")"
run "$D8" --version 0.45.5 --creds-unit k2-watch.service --creds-unit k2-ops-push.service
[ "$(cat "$ALLOWF")" = "$(printf 'k2-watch.service\nk2-control-plane.service\nk2-ops-push.service')" ] || fail "append without duplicates: $(cat "$ALLOWF")"
for bad in 'foo@.service' 'foo@bar.service' 'k2-.service' 'x' '../x.service' '-x.service'; do
	run "$WORK/box8b" --version 0.45.5 --creds-unit "$bad"
	[ "$rc" -ne 0 ] || fail "--creds-unit '$bad' accepted"
done
[ ! -e "$WORK/box8b/etc/k2-root-helper/systemd-cred-units" ] || fail "a refused --creds-unit wrote the list"
D9="$WORK/box9"
run "$D9" --version 0.45.5
[ ! -e "$D9/etc/k2-root-helper/systemd-cred-units" ] || fail "no --creds-unit must write no allow list (fail closed)"
ok "--creds-unit: appends, no duplicates, 0644, plain names only, absent by default"

echo ""
echo "PASS: install-root-helper ($pass checks)"
