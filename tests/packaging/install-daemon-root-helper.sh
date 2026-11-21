#!/usr/bin/env bash
# install-daemon.sh installs (or updates) the K2 root helper with the daemon,
# at the daemon's version, before the daemon binary is swapped; and
# provision-k2-server.sh relies on that one step (its 7a only checks).
#
# Hermetic: the REAL scripts/install-daemon.sh and scripts/install-root-helper.sh
# run against a temp K2_INSTALL_ROOT. PATH shims stand in for id, uname,
# getent, runuser, sudo, minisign, install, stat, visudo, dpkg-query, timeout
# and curl (github.com release URLs are served from a local mirror). The
# minisign shim "verifies" a daemon .sig2 carrying GOOD-DAEMON-SIG against any
# -P key (and, when that line also says SHA:<hex>, only the file with that
# sha256), and a root-helper .sig carrying GOOD-SIG against a pubkey file with
# the TEST key line; it prints the trusted comment like `minisign -V -Q`.
# No root, no network, no real sudo, nothing outside the temp dir.
# Run with: bash tests/packaging/install-daemon-root-helper.sh
# (K2_TEST_SH=dash runs the POSIX installer under dash, as Debian's sh.)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }

WORK="$(mktemp -d -t k2-install-daemon-rh-XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
SHIMS="$WORK/shims"
mkdir -p "$SHIMS"
REAL_INSTALL="$(command -v install)"
REAL_STAT="$(command -v stat)"
REAL_CURL="$(command -v curl)"
mode_of() { "$REAL_STAT" -c '%a' "$1" 2>/dev/null || "$REAL_STAT" -f '%Lp' "$1"; }
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

cat >"$SHIMS/id" <<'SH'
#!/bin/sh
case "${1:-}" in
    -u) echo "${SHIM_UID:-1001}" ;;
    -un) if [ "${SHIM_UID:-1001}" = 0 ]; then echo root; else echo "${SHIM_USER:-k2}"; fi ;;
    *) exit 0 ;;
esac
SH
cat >"$SHIMS/uname" <<'SH'
#!/bin/sh
case "${1:-}" in -s) echo Linux ;; -m) echo x86_64 ;; *) echo Linux ;; esac
SH
cat >"$SHIMS/getent" <<SH
#!/bin/sh
[ "\$1" = passwd ] && [ "\$2" = k2 ] && { echo "k2:x:1001:1001::$WORK/home/k2:/bin/bash"; exit 0; }
exit 2
SH
cat >"$SHIMS/runuser" <<SH
#!/bin/sh
[ "\$1" = "-u" ] && [ "\$3" = "--" ] || { echo "runuser shim: bad argv \$*" >&2; exit 97; }
echo "\$2 \$4" >>"$WORK/runuser.log"
shift 3
SHIM_UID=1001 exec "\$@"
SH
cat >"$SHIMS/sudo" <<SH
#!/bin/sh
# sudo -n -l <path>: may this user run it without a password (SHIM_DOOR).
if [ "\$1" = "-n" ] && [ "\$2" = "-l" ]; then exit "\${SHIM_DOOR:-1}"; fi
echo "sudo \$*" >>"$WORK/sudo.log"
if [ "\${SHIM_SUDO_FAIL:-0}" = 1 ]; then echo "k2 is not in the sudoers file." >&2; exit 1; fi
SHIM_UID=0 exec "\$@"
SH
cat >"$SHIMS/minisign" <<'SH'
#!/bin/sh
sig=""; pub=""; inline=""; msg=""
while [ $# -gt 0 ]; do
    case "$1" in -x) sig="$2"; shift 2 ;; -p) pub="$2"; shift 2 ;; -P) inline="$2"; shift 2 ;; -m) msg="$2"; shift 2 ;; *) shift ;; esac
done
if [ -n "$inline" ]; then
    grep -q GOOD-DAEMON-SIG "$sig" || exit 1
    want="$(sed -n 's/.*GOOD-DAEMON-SIG SHA:\([0-9a-f]*\).*/\1/p' "$sig")"
    if [ -n "$want" ]; then
        got="$( (sha256sum "$msg" 2>/dev/null || shasum -a 256 "$msg") | awk '{print $1}')"
        [ "$got" = "$want" ] || exit 1
    fi
else
    grep -q "^RWTESTKEY\$" "$pub" || exit 1
    grep -q GOOD-SIG "$sig" || exit 1
fi
sed -n 's/^trusted comment: //p' "$sig"
SH
cat >"$SHIMS/install" <<SH
#!/bin/sh
args=""
while [ \$# -gt 0 ]; do
    case "\$1" in
        -o|-g) shift 2 ;;
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
cat >"$SHIMS/visudo" <<'SH'
#!/bin/sh
exit 0
SH
cat >"$SHIMS/dpkg-query" <<'SH'
#!/bin/sh
exit "${SHIM_DPKG_OWNS:-1}"
SH
cat >"$SHIMS/timeout" <<'SH'
#!/bin/sh
shift
exec "$@"
SH
cat >"$SHIMS/curl" <<SH
#!/bin/sh
# Release URLs on github.com come from the mirror; everything else is real curl.
dest=""; url=""
for a in "\$@"; do :; done
prev=""
for a in "\$@"; do
    [ "\$prev" = "-o" ] && dest="\$a"
    case "\$a" in https://github.com/Alakazam-211/K2/releases/download/*) url="\$a" ;; esac
    prev="\$a"
done
if [ -n "\$url" ]; then
    echo "\$url" >>"$WORK/curl.log"
    src="$WORK/rel/\${url#https://github.com/Alakazam-211/K2/releases/download/}"
    [ -f "\$src" ] || exit 22
    if [ -n "\$dest" ]; then cp "\$src" "\$dest"; else cat "\$src"; fi
    exit 0
fi
exec "$REAL_CURL" "\$@"
SH
if ! command -v sha256sum >/dev/null 2>&1; then
	printf '#!/bin/sh\nexec shasum -a 256 "$@"\n' >"$SHIMS/sha256sum"
fi
chmod +x "$SHIMS"/*
export PATH="$SHIMS:$PATH"

V=0.46.13
DA="k2-daemon-linux-x86_64"
RA="k2-root-helper-linux-x86_64"
fake_helper() { # <out> <version> [protocol]
	{ printf '\177ELF'; printf 'pad\0'; printf 'K2_ROOT_HELPER_BUILD v=%s proto=%s arch=x86_64;' "$2" "${3:-13}"; printf '\0tail'; } >"$1"
}
release() { # <version>: daemon + manifest + root helper (+ .sig + .sha256) in rel/v<version>
	local v="$1" d="$WORK/rel/v$1"
	mkdir -p "$d"
	printf '#!/bin/sh\necho "k2-daemon %s"\n' "$v" >"$d/$DA"
	printf 'untrusted comment: test\nRUQGOOD-DAEMON-SIG\ntrusted comment: timestamp:1\tfile:%s\tversion:%s\nGLOBAL\n' "$DA" "$v" >"$d/$DA.sig2"
	cat >"$d/daemon-latest.json" <<JSON
{
  "version": "$v",
  "artifacts": {
    "linux-x86_64": {
      "url": "file://$d/$DA",
      "sha256": "$(sha "$d/$DA" | awk '{print $1}')",
      "sig2": "file://$d/$DA.sig2"
    }
  }
}
JSON
	fake_helper "$d/$RA" "$v"
	printf 'untrusted comment: test\nGOOD-SIG\ntrusted comment: file:%s version:%s protocol:13\nGLOBAL\n' "$RA" "$v" >"$d/$RA.sig"
	(cd "$d" && sha "$RA" >"$RA.sha256")
}
release "$V"

# The scripts under test, beside a wrapper that counts install-root-helper.sh
# runs (and execs the real one).
SD="$WORK/scripts"
mkdir -p "$SD"
cp "$ROOT/scripts/install-daemon.sh" "$SD/install-daemon.sh"
cp "$ROOT/scripts/install-root-helper.sh" "$SD/install-root-helper.real.sh"
cat >"$SD/install-root-helper.sh" <<SH
#!/usr/bin/env bash
echo "\$*" >>"$WORK/irh.log"
exec bash "$SD/install-root-helper.real.sh" "\$@"
SH

mkdir -p "$WORK/home/k2"
run() { # run <box> [args...]; sets out/rc. Env passes through.
	local box="$1"
	shift
	rm -f "$WORK/irh.log" "$WORK/sudo.log" "$WORK/runuser.log"
	set +e
	out="$(K2_INSTALL_ROOT="$box" K2_ROOT_HELPER_TEST_PUBKEYS=RWTESTKEY K2_APP_ENGINE_PATH="$WORK/no-engine" \
		HOME="$WORK/home/k2" "${K2_TEST_SH:-sh}" "$SD/install-daemon.sh" "$@" 2>&1)"
	rc=$?
	set -e
}
helper_v() { LC_ALL=C grep -a -o 'K2_ROOT_HELPER_BUILD v=[0-9.]*' "$1" | sed 's/.*v=//'; }
irh_runs() { [ -f "$WORK/irh.log" ] && grep -c . "$WORK/irh.log" || echo 0; }
BIN="$WORK/home/k2/.local/bin/k2-daemon"

echo "== as a normal user: the helper is installed with the daemon, through one sudo =="
B1="$WORK/box1"
rm -rf "$WORK/home/k2/.local" "$WORK/home/k2/.config"
run "$B1" --version "$V"
[ "$rc" -eq 0 ] || fail "install exit $rc: $out"
printf '%s\n' "$out" | sed 's/^/    | /'
[ "$(irh_runs)" = 1 ] || fail "install-root-helper.sh runs: $(irh_runs)"
grep -qx -- "--version $V --user k2 --unit k2-daemon.service --scope user" "$WORK/irh.log" \
	|| fail "installer argv: $(cat "$WORK/irh.log")"
grep -q "^sudo bash $SD/install-root-helper.sh --version $V " "$WORK/sudo.log" || fail "sudo argv: $(cat "$WORK/sudo.log")"
[ "$(grep -c . "$WORK/sudo.log")" = 1 ] || fail "more than one sudo: $(cat "$WORK/sudo.log")"
ok "one sudo runs install-root-helper.sh --version $V for k2 (k2-daemon.service, user scope: the unit this run writes)"
cmp -s "$WORK/rel/v$V/$RA" "$B1/usr/local/libexec/k2-root-helper" || fail "helper bytes are not the release asset"
[ "$(helper_v "$B1/usr/local/libexec/k2-root-helper")" = "$V" ] || fail "helper version"
cmp -s "$WORK/rel/v$V/$DA" "$BIN" || fail "daemon bytes"
"$BIN" | grep -qx "k2-daemon $V" || fail "daemon version"
ok "helper v$V (the signed release asset) and daemon v$V installed together: versions match"
grep -q '^scope = "user"$' "$B1/etc/k2-root-helper/daemon.toml" || fail "daemon.toml scope"
[ "$(mode_of "$B1/etc/sudoers.d/k2-root-helper")" = 440 ] || fail "sudoers mode"
<<<"$out" grep -q "Root helper: v$V installed for k2." || fail "summary line: $out"
<<<"$out" grep -q "minisign + sha256 verified" || fail "installer provenance: $out"
ok "daemon.toml names the user unit; sudoers 0440; the installer says minisign + sha256 verified"
L_HELPER="$(<<<"$out" grep -n 'Installing the K2 root helper' | cut -d: -f1)"
L_DAEMON="$(<<<"$out" grep -n "Installing to $BIN" | cut -d: -f1)"
[ -n "$L_HELPER" ] && [ -n "$L_DAEMON" ] && [ "$L_HELPER" -lt "$L_DAEMON" ] || fail "helper must go in before the daemon: $out"
ok "the helper goes in before the daemon binary is swapped"

echo "== the k5 program: verified like the daemon, before anything is installed =="
K5A="k5-cli-linux-x86_64"
K5="$WORK/home/k2/.local/bin/k5-cli"
FIRST_K5="$(sed -n 's/^K2_FIRST_K5_VERSION="\([0-9.]*\)"$/\1/p' "$SD/install-daemon.sh")"
[ -n "$FIRST_K5" ] || fail "install-daemon.sh has no K2_FIRST_K5_VERSION"
<<<"$out" grep -qF "The k5 program: v$V is older than $FIRST_K5, the first release that signs it; not installed" || fail "an older release must say k5 is skipped: $out"
[ ! -e "$K5" ] || fail "v$V has no signed k5 program, yet $K5 exists"
ok "v$V (older than $FIRST_K5): no k5 program fetched or installed, said so, the daemon installs (rc 0)"
V5="$FIRST_K5"
release "$V5"
k5_set() { # <sig2 form>: the release's k5-cli and its .sig2 (good, none, bad, other-file, other-version, other-bytes, legacy)
	local d="$WORK/rel/v$V5" file="$K5A" ver="$V5" alg=RUQ mark=GOOD-DAEMON-SIG sum
	{ printf '\177ELF'; printf 'K5_CLI_VERSION=%s\0' "$V5"; } >"$d/$K5A"
	rm -f "$d/$K5A.sig2"
	sum="$(sha "$d/$K5A" | awk '{print $1}')"
	case "$1" in
		none) return 0 ;;
		bad) mark=BAD-SIG ;;
		other-file) file="$DA" ;;
		other-version) ver=0.46.99 ;;
		other-bytes) sum="$(printf 'other bytes' | sha | awk '{print $1}')" ;;
		legacy) alg=RWQ ;;
		good) ;;
		*) fail "k5_set: unknown form $1" ;;
	esac
	printf 'untrusted comment: test\n%s%s SHA:%s\ntrusted comment: timestamp:1\tfile:%s\tversion:%s\nGLOBAL\n' "$alg" "$mark" "$sum" "$file" "$ver" >"$d/$K5A.sig2"
}
# k5_refused <label> <reason> <box> [run args...]: non-zero, the plain error
# names the file, and nothing at all was installed (no helper run, no daemon,
# no k5 program, no unit).
k5_refused() {
	local label="$1" why="$2" box="$WORK/box-k5-$3"
	shift 3
	rm -rf "$WORK/home/k2/.local" "$WORK/home/k2/.config" "$box"
	run "$box" "$@"
	[ "$rc" -ne 0 ] || fail "$label: installed (rc 0): $out"
	<<<"$out" grep -qF "K5 PROGRAM NOT VERIFIED: " || fail "$label: no K5 PROGRAM NOT VERIFIED line: $out"
	<<<"$out" grep -qF "$why" || fail "$label: the reason must say '$why': $out"
	<<<"$out" grep -qF "Nothing was installed or changed" || fail "$label: must say nothing was installed: $out"
	[ "$(irh_runs)" = 0 ] && [ ! -e "$WORK/sudo.log" ] || fail "$label: the root helper step ran"
	[ ! -e "$box/usr/local/libexec/k2-root-helper" ] || fail "$label: a root helper was installed"
	[ ! -e "$BIN" ] && [ ! -e "$K5" ] && [ ! -e "$K5.new" ] || fail "$label: a daemon or k5 program was installed"
	[ ! -e "$WORK/home/k2/.config/systemd/user/k2-daemon.service" ] || fail "$label: a service unit was written"
	ok "v$V5, $label: refused, the error names $K5A, nothing installed"
}
rm -f "$WORK/rel/v$V5/$K5A" "$WORK/rel/v$V5/$K5A.sig2"
k5_refused "no k5 program in the release" "$K5A could not be fetched" none --version "$V5"
k5_set none
k5_refused "an unsigned k5 program (no .sig2)" "$K5A.sig2 could not be fetched" unsigned --version "$V5"
k5_set bad
k5_refused "a bad signature" "MINISIGN VERIFICATION FAILED for $K5A" bad --version "$V5"
k5_set other-bytes
k5_refused "a tampered k5 program (a good signature for other bytes)" "MINISIGN VERIFICATION FAILED for $K5A" tampered --version "$V5"
k5_set other-file
k5_refused "a signature for another file (the daemon's)" "The signature is for '$DA', not $K5A" otherfile --version "$V5"
k5_set other-version
k5_refused "a signature for another version" "The signature is for version '0.46.99', not $V5" otherver --version "$V5"
k5_set legacy
k5_refused "a legacy (not prehashed) signature" "not a prehashed minisign signature" legacy --version "$V5"
k5_set none
k5_refused "air-gapped (file://) without the .sig2 beside it" "$K5A.sig2 could not be fetched" gap --no-service --manifest-url "file://$WORK/rel/v$V5/daemon-latest.json"
k5_set bad
SHIM_UID=0 k5_refused "a bad signature, run as root for k2" "MINISIGN VERIFICATION FAILED for $K5A" root --version "$V5" --user k2
# Nothing in the environment turns the check off: the keys are the script's
# own (K2_DAEMON_PUBKEYS is never read from it) and no variable skips it.
K2_DAEMON_PUBKEYS=RWANYKEY K2_NO_K5=1 K2_SKIP_K5=1 K2_K5_VERIFY=0 K2_NO_VERIFY=1 \
	k5_refused "a bad signature with look-alike skip variables set" "MINISIGN VERIFICATION FAILED for $K5A" env --version "$V5"
[ "$(grep -c '^K2_DAEMON_PUBKEYS="RW[A-Za-z0-9+/ ]*"$' "$SD/install-daemon.sh")" = 1 ] \
	&& [ "$(grep -c 'K2_DAEMON_PUBKEYS=' "$SD/install-daemon.sh")" = 1 ] || fail "K2_DAEMON_PUBKEYS must be set once, as a pinned literal"
ok "the update keys are one pinned literal in the script, never from the environment"
# The verifier itself missing: refused before anything is fetched or installed.
NOMS="$WORK/no-minisign"
mkdir -p "$NOMS"
for f in "$SHIMS"/* /usr/local/bin/* /usr/bin/* /bin/*; do
	b="${f##*/}"
	[ "$b" = minisign ] || [ -e "$NOMS/$b" ] || ln -sf "$f" "$NOMS/$b"
done
rm -f "$NOMS/minisign"
k5_set good
rm -rf "$WORK/home/k2/.local" "$WORK/box-k5-noms"
PATH="$NOMS" run "$WORK/box-k5-noms" --version "$V5"
[ "$rc" -ne 0 ] && <<<"$out" grep -q "minisign is required" && <<<"$out" grep -q "Refusing to install an unverified binary" || fail "no minisign must refuse: rc=$rc $out"
[ ! -e "$BIN" ] && [ ! -e "$K5" ] && [ "$(irh_runs)" = 0 ] || fail "no minisign: something was installed"
ok "no minisign on PATH: refused before any download, nothing installed"
rm -rf "$WORK/home/k2/.local" "$WORK/home/k2/.config" "$WORK/curl.log"
run "$WORK/box-k5-good" --version "$V5"
[ "$rc" -eq 0 ] || fail "a good k5 program: $out"
cmp -s "$WORK/rel/v$V5/$K5A" "$K5" || fail "k5-cli bytes are not the release asset"
[ "$(mode_of "$K5")" = 755 ] || fail "k5-cli mode $(mode_of "$K5")"
[ "$(dirname "$K5")" = "$(dirname "$BIN")" ] || fail "k5-cli is not beside the daemon"
[ ! -L "$WORK/home/k2/.local/bin/k2" ] && [ ! -e "$WORK/home/k2/.local/bin/k5" ] || fail "the installer linked k2 or k5 (only the switch may)"
<<<"$out" grep -qF "Installed $K5, minisign-verified" || fail "k5 line: $out"
grep -qx "https://github.com/Alakazam-211/K2/releases/download/v$V5/$K5A.sig2" "$WORK/curl.log" || fail "the .sig2 was not fetched from the same release: $(cat "$WORK/curl.log")"
L_K5V="$(<<<"$out" grep -n "Verifying the k5 program's minisign signature" | cut -d: -f1)"
L_HELPER="$(<<<"$out" grep -n 'Installing the K2 root helper' | cut -d: -f1)"
[ -n "$L_K5V" ] && [ -n "$L_HELPER" ] && [ "$L_K5V" -lt "$L_HELPER" ] || fail "the k5 check must come before the root helper and the daemon: $out"
ok "a good signature: k5-cli (the release asset, 0755) beside the daemon, checked before the helper and daemon go in; no k2 or k5 link"
rm -rf "$WORK/home/k2/.local"
SHIM_UID=0 run "$WORK/box-k5-root" --user k2 --no-service --version "$V5"
[ "$rc" -eq 0 ] && cmp -s "$WORK/rel/v$V5/$K5A" "$K5" || fail "root mode with a good k5 program: $out"
[ "$(grep -c '^k2 cp$' "$WORK/runuser.log")" -ge 2 ] || fail "the daemon and k5-cli must both be written as k2 in root mode: $(cat "$WORK/runuser.log")"
ok "as root for k2: the verified k5 program is written as k2"
run "$WORK/box-k5-dry" --version "$V5" --dry-run
<<<"$out" grep -qF "k5 program:     https://github.com/Alakazam-211/K2/releases/download/v$V5/$K5A (+ .sig2: K2 update key, file:$K5A version:$V5)" || fail "dry run k5 line: $out"
ok "dry run names the k5 program and the signature it will need"

echo "== the first k5 release: 0.46.16 published no k5 program, 0.46.17 does =="
# Pinned versions, not read from the script: v0.46.15 and v0.46.16 went out
# with no k5-cli asset, so an install of either must not ask for one.
# install_pinned <version> <box>: a fresh home, the curl log emptied, then
# --version <version>.
install_pinned() {
	rm -rf "$WORK/home/k2/.local" "$WORK/home/k2/.config" "$WORK/curl.log"
	run "$2" --version "$1"
}
release 0.46.16
# As published: no k5-cli and no .sig2 in v0.46.16.
rm -f "$WORK/rel/v0.46.16/$K5A" "$WORK/rel/v0.46.16/$K5A.sig2"
install_pinned 0.46.16 "$WORK/box-k5-0.46.16"
[ "$rc" -eq 0 ] || fail "a pinned 0.46.16 (no k5-cli published) must install: rc=$rc $out"
! grep -q "/$K5A" "$WORK/curl.log" || fail "a pinned 0.46.16 asked for $K5A: $(cat "$WORK/curl.log")"
[ ! -e "$K5" ] || fail "a pinned 0.46.16 installed $K5"
cmp -s "$WORK/rel/v0.46.16/$DA" "$BIN" || fail "a pinned 0.46.16: the daemon is not the release asset"
<<<"$out" grep -qF "The k5 program: v0.46.16 is older than 0.46.17, the first release that signs it; not installed" || fail "a pinned 0.46.16 must say k5 is skipped: $out"
ok "pinned 0.46.16: no $K5A fetched or installed, the daemon installs (rc 0)"
release 0.46.17
d17="$WORK/rel/v0.46.17"
{ printf '\177ELF'; printf 'K5_CLI_VERSION=0.46.17\0'; } >"$d17/$K5A"
printf 'untrusted comment: test\nRUQGOOD-DAEMON-SIG SHA:%s\ntrusted comment: timestamp:1\tfile:%s\tversion:0.46.17\nGLOBAL\n' \
	"$(sha "$d17/$K5A" | awk '{print $1}')" "$K5A" >"$d17/$K5A.sig2"
install_pinned 0.46.17 "$WORK/box-k5-0.46.17"
[ "$rc" -eq 0 ] || fail "a pinned 0.46.17 with a good k5 program: rc=$rc $out"
grep -qx "https://github.com/Alakazam-211/K2/releases/download/v0.46.17/$K5A" "$WORK/curl.log" \
	&& grep -qx "https://github.com/Alakazam-211/K2/releases/download/v0.46.17/$K5A.sig2" "$WORK/curl.log" \
	|| fail "a pinned 0.46.17 must fetch $K5A and its .sig2: $(cat "$WORK/curl.log")"
<<<"$out" grep -qF "Verifying the k5 program's minisign signature" && <<<"$out" grep -qF "Installed $K5, minisign-verified" || fail "a pinned 0.46.17 must verify and install k5: $out"
cmp -s "$d17/$K5A" "$K5" || fail "a pinned 0.46.17: k5-cli is not the release asset"
ok "pinned 0.46.17: $K5A and its .sig2 fetched, verified and installed beside the daemon"

echo "== re-run with the helper current: nothing to change, no sudo =="
SHIM_DOOR=0 run "$B1" --version "$V"
[ "$rc" -eq 0 ] || fail "re-run: $out"
[ ! -e "$WORK/sudo.log" ] && [ "$(irh_runs)" = 0 ] || fail "re-run must not sudo or reinstall"
<<<"$out" grep -q "Root helper: v$V is installed for k2 (k2-daemon.service, user scope); nothing to change." || fail "current line: $out"
ok "current helper + its sudoers line: no sudo prompt, no second install"
run "$B1" --version "$V"
[ "$(irh_runs)" = 1 ] || fail "a missing sudo door must reinstall (idempotent)"
ok "without its sudoers door the step runs the (idempotent) installer again"

echo "== signature failure: loud, and neither the helper nor the daemon changes =="
B3="$WORK/box3"
rm -rf "$WORK/home/k2/.local"
cp "$WORK/rel/v$V/$RA.sig" "$WORK/good.sig"
sed -i.bak 's/GOOD-SIG/BAD-SIG/' "$WORK/rel/v$V/$RA.sig"
run "$B3" --version "$V"
mv "$WORK/good.sig" "$WORK/rel/v$V/$RA.sig"
[ "$rc" -ne 0 ] || fail "a bad helper signature must fail: $out"
<<<"$out" grep -q "MINISIGN VERIFICATION FAILED" || fail "installer reason: $out"
<<<"$out" grep -q "ROOT HELPER NOT INSTALLED" || fail "install-daemon.sh message: $out"
<<<"$out" grep -q "The daemon was not installed or changed" || fail "says the daemon is unchanged: $out"
[ ! -e "$B3/usr/local/libexec/k2-root-helper" ] || fail "helper installed despite a bad signature"
[ ! -e "$BIN" ] || fail "daemon installed despite the helper failure"
ok "bad signature: non-zero, clear message, no helper, daemon not installed"

echo "== the fleet's non-root form (no sudo rights): missing helper is loud, daemon unchanged =="
B4="$WORK/box4"
SHIM_SUDO_FAIL=1 run "$B4" --version "$V" --no-service
[ "$rc" -ne 0 ] || fail "sudo failure must fail: $out"
<<<"$out" grep -q "ROOT HELPER NOT INSTALLED" || fail "message: $out"
<<<"$out" grep -q "sudo sh install-daemon.sh --user k2 --version $V" || fail "names the root command: $out"
[ ! -e "$BIN" ] || fail "daemon installed despite the helper failure"
ok "missing helper, sudo refused: non-zero, names the one root command, daemon not installed"

echo "== the fleet's non-root form: an older healthy helper is left to auto-heal =="
old_box() { # <box> <version> <protocol> <toml user>
	mkdir -p "$1/usr/local/libexec" "$1/etc/k2-root-helper"
	fake_helper "$1/usr/local/libexec/k2-root-helper" "$2" "$3"
	printf 'user = "%s"\nunit = "k2-daemon.service"\nscope = "system"\n' "$4" >"$1/etc/k2-root-helper/daemon.toml"
}
B4b="$WORK/box4b"
old_box "$B4b" 0.46.12 13 k2
rm -rf "$WORK/home/k2/.local"
SHIM_DOOR=0 SHIM_SUDO_FAIL=1 run "$B4b" --version "$V" --no-service
[ "$rc" -eq 0 ] || fail "older healthy helper must not stop the roll: $out"
[ ! -e "$WORK/sudo.log" ] && [ "$(irh_runs)" = 0 ] || fail "no sudo, no install for an auto-healable helper"
<<<"$out" grep -qF "Root helper: v0.46.12 is older; the daemon will update it after it starts (auto-heal). To do it now: sudo sh install-daemon.sh --user k2 --version $V" \
	|| fail "the auto-heal note: $out"
cmp -s "$WORK/rel/v$V/$DA" "$BIN" || fail "the daemon must still install"
[ "$(helper_v "$B4b/usr/local/libexec/k2-root-helper")" = 0.46.12 ] || fail "helper changed"
ok "older, root-owned, protocol 13, names k2, door open: daemon installs, one note, helper left to auto-heal"
for bad in "legacy:0.45.0:2:k2:0" "not_allowed:0.46.12:13:k2:1" "other user:0.46.12:13:acme:0"; do
	IFS=: read -r label ver proto who door <<<"$bad"
	box="$WORK/box4-${label// /-}"
	old_box "$box" "$ver" "$proto" "$who"
	rm -rf "$WORK/home/k2/.local"
	SHIM_DOOR="$door" SHIM_SUDO_FAIL=1 run "$box" --version "$V" --no-service
	[ "$rc" -ne 0 ] && <<<"$out" grep -q "ROOT HELPER NOT INSTALLED" || fail "$label helper must fail loudly: $out"
	[ ! -e "$BIN" ] || fail "$label: daemon installed despite the helper"
done
ok "legacy (protocol 2), not allowed by sudoers, or named for another user: loud, daemon unchanged"
B4c="$WORK/box4c"
mkdir -p "$B4c/usr/local/libexec"
ln -s "$B4c/usr/lib/k2/gone" "$B4c/usr/local/libexec/k2-root-helper"
SHIM_SUDO_FAIL=1 run "$B4c" --version "$V"
[ "$rc" -ne 0 ] && <<<"$out" grep -q "a link to a file that is gone" || fail "broken package link: $out"
[ ! -e "$BIN" ] || fail "broken package link: daemon installed"
ok "a broken package-owned link: loud, daemon unchanged"

echo "== a package-owned helper is left alone =="
B5="$WORK/box5"
mkdir -p "$B5/usr/lib/k2" "$B5/usr/local/libexec"
fake_helper "$B5/usr/lib/k2/k2-root-helper" 0.46.1
ln -s "$B5/usr/lib/k2/k2-root-helper" "$B5/usr/local/libexec/k2-root-helper"
run "$B5" --version "$V"
[ "$rc" -eq 0 ] || fail "package-owned: $out"
[ ! -e "$WORK/sudo.log" ] && [ "$(irh_runs)" = 0 ] || fail "package-owned helper must not be touched"
[ -L "$B5/usr/local/libexec/k2-root-helper" ] && [ "$(helper_v "$B5/usr/lib/k2/k2-root-helper")" = 0.46.1 ] || fail "package helper changed"
<<<"$out" grep -q "managed by your package manager; left alone" || fail "message: $out"
cmp -s "$WORK/rel/v$V/$DA" "$BIN" || fail "the daemon still installs"
B5b="$WORK/box5b"
SHIM_DPKG_OWNS=0 run "$B5b" --version "$V"
[ "$rc" -eq 0 ] && [ "$(irh_runs)" = 0 ] || fail "dpkg-owned helper must not be touched: $out"
ok "a symlinked (package) helper or one dpkg owns: left alone, the daemon still installs"

echo "== as root with --user: daemon files written as k2, helper installed directly =="
B6="$WORK/box6"
mkdir -p "$B6/etc/k2-root-helper"
printf '[[daemon]]\nuser = "k2"\nunit = "k2-daemon-std.service"\nscope = "system"\n' >"$B6/etc/k2-root-helper/daemon.toml"
rm -rf "$WORK/home/k2/.local"
SHIM_UID=0 run "$B6" --user k2 --no-service --version "$V"
[ "$rc" -eq 0 ] || fail "root mode: $out"
[ ! -e "$WORK/sudo.log" ] || fail "root mode must not use sudo"
grep -qx -- "--version $V --user k2 --unit k2-daemon-std.service --scope system" "$WORK/irh.log" \
	|| fail "root-mode argv (keeps the box's unit + scope): $(cat "$WORK/irh.log")"
grep -q '^k2 cp$' "$WORK/runuser.log" && grep -q '^k2 mv$' "$WORK/runuser.log" || fail "daemon files must be written as k2: $(cat "$WORK/runuser.log")"
cmp -s "$WORK/rel/v$V/$DA" "$BIN" || fail "root-mode daemon bytes"
[ "$(helper_v "$B6/usr/local/libexec/k2-root-helper")" = "$V" ] || fail "root-mode helper version"
ok "--user k2 as root: one run, daemon written as k2, helper v$V for the unit daemon.toml already names"
SHIM_UID=1001 SHIM_USER=alice run "$B6" --user k2 --version "$V"
[ "$rc" -ne 0 ] && <<<"$out" grep -q "needs root" || fail "--user for another user without root: $out"
ok "--user for someone else without root is refused"

echo "== an air-gapped (file://) manifest: the signed helper from the same folder =="
B7="$WORK/box7"
rm -rf "$WORK/home/k2/.local" "$WORK/curl.log"
run "$B7" --no-service --manifest-url "file://$WORK/rel/v$V/daemon-latest.json"
[ "$rc" -eq 0 ] || fail "air-gap: $out"
grep -qx -- "--file $WORK/rel/v$V/$RA --user k2 --unit k2-daemon.service --scope system" "$WORK/irh.log" \
	|| fail "air-gap argv: $(cat "$WORK/irh.log")"
[ ! -e "$WORK/curl.log" ] || fail "air-gap must not fetch: $(cat "$WORK/curl.log")"
<<<"$out" grep -q "minisign verified" || fail "the local helper's .sig is checked: $out"
ok "file:// manifest: helper + .sig from the manifest's folder, no network"
mkdir -p "$WORK/gap"
cp "$WORK/rel/v$V/$DA" "$WORK/rel/v$V/$DA.sig2" "$WORK/rel/v$V/daemon-latest.json" "$WORK/rel/v$V/$RA" "$WORK/gap/"
run "$WORK/box7b" --no-service --manifest-url "file://$WORK/rel/v$V/daemon-latest.json" --root-helper-from "$WORK/gap"
[ "$rc" -ne 0 ] && <<<"$out" grep -q "$RA.sig was not found" || fail "unsigned air-gap helper must fail: $out"
ok "an air-gap helper without its .sig: refused"
fake_helper "$WORK/gap/$RA" 0.46.12
printf 'x' >"$WORK/gap/$RA.sig"
run "$WORK/box7c" --no-service --manifest-url "file://$WORK/rel/v$V/daemon-latest.json" --root-helper-from "$WORK/gap"
[ "$rc" -ne 0 ] && <<<"$out" grep -q "is v0.46.12, but the daemon is v$V; they must match" || fail "version mismatch: $out"
[ ! -e "$WORK/box7c/usr/local/libexec/k2-root-helper" ] || fail "mismatched helper installed"
ok "a helper of another version: refused before any root step"

echo "== a newer helper stays; --no-root-helper leaves it alone; dry run shows the plan =="
B8="$WORK/box8"
mkdir -p "$B8/usr/local/libexec" "$B8/etc/k2-root-helper"
fake_helper "$B8/usr/local/libexec/k2-root-helper" 0.47.0
printf 'user = "k2"\nunit = "k2-daemon.service"\nscope = "user"\n' >"$B8/etc/k2-root-helper/daemon.toml"
SHIM_DOOR=0 run "$B8" --version "$V"
[ "$rc" -eq 0 ] && [ "$(irh_runs)" = 0 ] || fail "newer helper: $out"
[ "$(helper_v "$B8/usr/local/libexec/k2-root-helper")" = 0.47.0 ] || fail "newer helper downgraded"
ok "a newer helper is not downgraded"
run "$WORK/box9" --version "$V" --no-root-helper
[ "$rc" -eq 0 ] && [ "$(irh_runs)" = 0 ] && <<<"$out" grep -q "Root helper: not touched (--no-root-helper)." || fail "--no-root-helper: $out"
ok "--no-root-helper: not touched, said so"
run "$WORK/box10" --version "$V" --dry-run
[ "$rc" -eq 0 ] && [ "$(irh_runs)" = 0 ] || fail "dry run: $out"
<<<"$out" grep -q "root helper:    /usr/local/libexec/k2-root-helper v$V for k2 (k2-daemon.service, user scope), before the daemon" || fail "dry-run plan: $out"
<<<"$out" grep -q "helper install: $SD/install-root-helper.sh, through sudo" || fail "dry-run installer: $out"
ok "dry run prints the helper plan and changes nothing"

echo "== --version takes the release with or without its v: one URL, never vv =="
# The manifest URL adds the tag's v, so v$V once built .../download/vv$V/.
# The curl shim logs every release URL; a vv URL has no file and fails.
WANT_URL="https://github.com/Alakazam-211/K2/releases/download/v$V/daemon-latest.json"
n=0
for how in "flag:$V" "flag:v$V" "env:$V" "env:v$V"; do
	n=$((n + 1))
	given="${how#*:}"
	rm -f "$WORK/curl.log"
	case "$how" in
		flag:*) run "$WORK/box-ver$n" --version "$given" --dry-run ;;
		env:*) K2_VERSION="$given" run "$WORK/box-ver$n" --dry-run ;;
	esac
	label="${how%%:*} $given"
	[ "$rc" -eq 0 ] || fail "$label: exit $rc: $out"
	[ -f "$WORK/curl.log" ] || fail "$label: no release request"
	[ "$(head -n1 "$WORK/curl.log")" = "$WANT_URL" ] || fail "$label: first request $(head -n1 "$WORK/curl.log"), want $WANT_URL"
	! grep -q '/download/vv' "$WORK/curl.log" || fail "$label: asked for a vv URL: $(cat "$WORK/curl.log")"
	<<<"$out" grep -qx "  requested ver:  $V" || fail "$label: dry run's requested version: $out"
	ok "$label: asks for $WANT_URL and plans v$V"
done

echo "== provisioning: install-daemon.sh installs the helper once; step 7a only checks =="
PROVISION="$ROOT/scripts/provision-k2-server.sh"
STEP4="$(sed -n '/^# ── 4\. daemon binary/,/^# ── 5\./p' "$PROVISION")"
STEP7A="$(sed -n '/^# ── 7a\. root helper/,/^# ── 7b\./p' "$PROVISION")"
[ -n "$STEP4" ] && [ -n "$STEP7A" ] || fail "provision blocks not found"
STEP="$WORK/provision-steps.sh"
{
	echo 'set -euo pipefail'
	echo 'log() { printf "[provision] %s\n" "$*"; }'
	echo 'die() { printf "[provision] ERROR: %s\n" "$*" >&2; exit 1; }'
	printf '%s\n' "$STEP4" "$STEP7A"
	echo 'echo "SUMMARY=${MAIL_HELPER_LINE}"'
} >"$STEP"
B11="$WORK/box11"
rm -rf "$WORK/home/k2/.local" "$WORK/irh.log" "$WORK/sudo.log"
set +e
out="$(SHIM_UID=0 K2_INSTALL_ROOT="$B11" K2_ROOT_HELPER_TEST_PUBKEYS=RWTESTKEY K2_APP_ENGINE_PATH="$WORK/no-engine" \
	SCRIPT_DIR="$SD" K2_RUN_USER=k2 K2_UNIT_NAME=k2-daemon K2_HOME="$WORK/home/k2" K2_VERSION="$V" \
	RAW_BASE="file:///nonexistent" bash "$STEP" 2>&1)"
rc=$?
set -e
[ "$rc" -eq 0 ] || fail "provision steps 4 + 7a: $out"
printf '%s\n' "$out" | grep -E '^\[provision\]|SUMMARY' | sed 's/^/    | /'
[ "$(irh_runs)" = 1 ] || fail "install-root-helper.sh ran $(irh_runs) times in provisioning"
grep -qx -- "--version $V --user k2 --unit k2-daemon.service --scope system" "$WORK/irh.log" || fail "provision argv: $(cat "$WORK/irh.log")"
[ ! -e "$WORK/sudo.log" ] || fail "provisioning (root) must not use sudo"
<<<"$out" grep -q "SUMMARY=installed (v$V)" || fail "7a summary: $out"
ok "provisioning installs the helper exactly once (install-daemon.sh as root, system unit); 7a checks it"
[ -f "$B11/etc/k2-root-helper/daemon.toml" ] && [ -f "$B1/etc/k2-root-helper/daemon.toml" ] || fail "toml"
diff <(sed 's/^scope = .*//' "$B11/etc/k2-root-helper/daemon.toml") <(sed 's/^scope = .*//' "$B1/etc/k2-root-helper/daemon.toml") >/dev/null \
	|| fail "the two routes write different daemon.toml shapes"
cmp -s "$B11/usr/local/libexec/k2-root-helper" "$B1/usr/local/libexec/k2-root-helper" \
	&& cmp -s "$B11/etc/sudoers.d/k2-root-helper" "$B1/etc/sudoers.d/k2-root-helper" || fail "the two routes differ"
ok "both routes end with the same helper bytes and sudoers line (scope follows the unit each one runs)"
# provision-k2-server.sh (and bootstrap-k2-dedicated.sh, which runs it)
# installs through install-daemon.sh: a pinned 0.46.16 there must not ask
# for the k5 program either.
B12="$WORK/box12"
rm -rf "$WORK/home/k2/.local" "$WORK/irh.log" "$WORK/sudo.log" "$WORK/curl.log"
set +e
out="$(SHIM_UID=0 K2_INSTALL_ROOT="$B12" K2_ROOT_HELPER_TEST_PUBKEYS=RWTESTKEY K2_APP_ENGINE_PATH="$WORK/no-engine" \
	SCRIPT_DIR="$SD" K2_RUN_USER=k2 K2_UNIT_NAME=k2-daemon K2_HOME="$WORK/home/k2" K2_VERSION=0.46.16 \
	RAW_BASE="file:///nonexistent" bash "$STEP" 2>&1)"
rc=$?
set -e
[ "$rc" -eq 0 ] || fail "provision steps 4 + 7a at 0.46.16: $out"
! grep -q "/$K5A" "$WORK/curl.log" || fail "provisioning 0.46.16 asked for $K5A: $(cat "$WORK/curl.log")"
[ ! -e "$K5" ] && cmp -s "$WORK/rel/v0.46.16/$DA" "$BIN" || fail "provisioning 0.46.16: k5 installed or the daemon missing"
<<<"$out" grep -q "SUMMARY=installed (v0.46.16)" || fail "provisioning 0.46.16, 7a summary: $out"
ok "provisioning a pinned 0.46.16: no $K5A fetched, the daemon and helper install"

echo ""
echo "PASS: install-daemon.sh root helper ($pass checks)"
