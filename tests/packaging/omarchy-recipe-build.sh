#!/usr/bin/env bash
# Build the k2-bin recipe for real, on an Arch machine, the way Omarchy's PR
# check does (PRD test 13). Not a CI step: it downloads the recipe's release
# package and needs Arch tools. Missing tools fail; nothing is skipped.
#
#   bash tests/packaging/omarchy-recipe-build.sh
#
# Proves:
#   - makepkg builds it, and the checksum check runs: a wrong sha256 fails;
#   - the package lists exactly the files of the release package it unpacks,
#     with the same modes, and the paths in packaging/arch/README.md's table;
#   - its metadata (provides, conflicts, install file, no build tools);
#   - namcap reports no errors (K2_OMARCHY_NO_NAMCAP=1 where it can't be
#     installed; the output then says NOT RUN);
#   - a pacman install into a scratch root works, and the installed daemon and
#     CLI start and report the version;
#   - omacom/omarchy-pkgs' own checks pass on a copy of the recipe; with Docker
#     or Podman, its bin/build does too (the output says which ran).
#
# Environment: K2_OMARCHY_PKGS_REF, the omarchy-pkgs commit to test against
# (default master).
set -euo pipefail
export LC_ALL=C

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RECIPE="$ROOT/packaging/omarchy/k2-bin"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
npass=0 nfail=0
pass() { echo "  ok: $1"; npass=$((npass + 1)); }
fail() { echo "  FAIL: $1" >&2; [ -z "${2:-}" ] || echo "        $2" >&2; nfail=$((nfail + 1)); }

for t in makepkg pacman bsdtar fakeroot python3 jq; do
    command -v "$t" >/dev/null || { echo "FAIL: $t is not installed (this test runs on Arch with base-devel and jq)" >&2; exit 1; }
done
# namcap is required, except where a person has said this run goes without it:
# the result then says NOT RUN and can't be read as a pass of that check.
if ! command -v namcap >/dev/null && [ "${K2_OMARCHY_NO_NAMCAP:-}" != 1 ]; then
    echo "FAIL: namcap is not installed (K2_OMARCHY_NO_NAMCAP=1 runs the rest and reports it as not run)" >&2
    exit 1
fi

# Every makepkg uses the same settings, with no system-wide colour or PACKAGER.
mp() { env -u SHELLOPTS -u MAKEFLAGS PKGEXT=.pkg.tar.zst makepkg --nocolor --nodeps "$@"; }

echo "== build =="
cp -R "$RECIPE" "$tmp/k2-bin"
. "$RECIPE/PKGBUILD"
ver="$pkgver"
if (cd "$tmp/k2-bin" && mp -f) > "$tmp/build.log" 2>&1; then pass "makepkg builds k2-bin $ver"; else fail "makepkg" "$(tail -30 "$tmp/build.log")"; exit 1; fi
grep -q 'Validating source files with sha256sums' "$tmp/build.log" && pass "makepkg validated the sums" || fail "no validation line" "$(cat "$tmp/build.log")"
PKG="$tmp/k2-bin/k2-bin-${ver}-1-x86_64.pkg.tar.zst"
[ -s "$PKG" ] && pass "made $(basename "$PKG") ($(du -h "$PKG" | cut -f1))" || { fail "no package at $PKG" "$(ls -la "$tmp/k2-bin")"; exit 1; }
INNER="$tmp/k2-bin/k2-${ver}-x86_64.pkg.tar.zst"
[ -s "$INNER" ] || { fail "the downloaded release package is not at $INNER"; exit 1; }

echo "== contents =="
listing() { bsdtar -tvf "$1" | grep -vE ' \.(PKGINFO|BUILDINFO|MTREE|INSTALL)$' | awk '{ m=$1; $1=$2=$3=$4=$5=$6=$7=$8=""; sub(/^ +/, ""); print m, $0 }' | sort; }
listing "$PKG" > "$tmp/outer.list"
listing "$INNER" > "$tmp/inner.list"
if diff "$tmp/inner.list" "$tmp/outer.list" > "$tmp/list.diff"; then
    pass "k2-bin holds exactly the release package's $(wc -l < "$tmp/outer.list") paths, same modes"
else
    fail "k2-bin's files differ from the release package" "$(head -30 "$tmp/list.diff")"
fi
# The README's path table.
sed -n '/^## Paths/,/^## /p' "$ROOT/packaging/arch/README.md" | grep -oE '^\| `/[^`]+`' | sed 's/^| `\///; s/`$//' > "$tmp/readme.paths"
[ "$(wc -l < "$tmp/readme.paths")" -ge 7 ] || fail "the README path table parsed to too few rows" "$(cat "$tmp/readme.paths")"
missing=0
while read -r p; do
    grep -qE " ${p}( -> .*)?\$" "$tmp/outer.list" || { fail "README lists /$p; the package doesn't have it"; missing=1; }
done < "$tmp/readme.paths"
[ "$missing" = 0 ] && pass "every path in packaging/arch/README.md's table is in the package"
# And the other way: every file under bin and lib is in the table.
grep -vE '/$' "$tmp/outer.list" | awk '{print $2}' | grep -E '^usr/(bin|lib/k2|lib/systemd)/' | sort > "$tmp/pkg.paths"
sort "$tmp/readme.paths" > "$tmp/readme.sorted"
if extra="$(comm -13 "$tmp/readme.sorted" "$tmp/pkg.paths")" && [ -z "$extra" ]; then
    pass "every program and unit in the package is in the README's table"
else
    fail "in the package, not in the README's table" "$extra"
fi

echo "== metadata =="
bsdtar -xOf "$PKG" .PKGINFO > "$tmp/pkginfo"
for want in "pkgname = k2-bin" "pkgver = ${ver}-1" "provides = k2=${ver}" "conflict = k2" "license = ${license[0]}" "arch = x86_64"; do
    grep -qxF "$want" "$tmp/pkginfo" && pass "PKGINFO: $want" || fail "PKGINFO has no '$want'" "$(cat "$tmp/pkginfo")"
done
if grep -q '^makedepend' "$tmp/pkginfo"; then fail "k2-bin has build dependencies" "$(grep '^makedepend' "$tmp/pkginfo")"; else pass "no build dependencies"; fi
bsdtar -xOf "$PKG" .INSTALL | cmp -s - "$ROOT/packaging/arch/k2.install" && pass "the install hooks are ours" || fail "the .INSTALL differs from packaging/arch/k2.install"
# A strip or debug pass would change the bytes: the binaries must be identical.
mkdir -p "$tmp/x/outer" "$tmp/x/inner"
bsdtar -xf "$PKG" -C "$tmp/x/outer" usr/bin/k2-daemon usr/lib/k2/k2-gui
bsdtar -xf "$INNER" -C "$tmp/x/inner" usr/bin/k2-daemon usr/lib/k2/k2-gui
cmp -s "$tmp/x/outer/usr/bin/k2-daemon" "$tmp/x/inner/usr/bin/k2-daemon" && cmp -s "$tmp/x/outer/usr/lib/k2/k2-gui" "$tmp/x/inner/usr/lib/k2/k2-gui" \
    && pass "the daemon and GUI binaries are byte-identical to the release package's" || fail "binaries changed in repackaging"

echo "== namcap =="
if command -v namcap >/dev/null; then
    namc=0
    namcap "$PKG" > "$tmp/namcap.out" 2>&1 || namc=$?
    sed 's/^/    /' "$tmp/namcap.out"
    [ "$namc" = 0 ] || fail "namcap exited $namc"
    if grep -E '^k2-bin E: ' "$tmp/namcap.out" > "$tmp/namcap.err"; then fail "namcap reports errors" "$(cat "$tmp/namcap.err")"; else pass "namcap: no errors"; fi
else
    echo "  NOT RUN: namcap (K2_OMARCHY_NO_NAMCAP=1)"
fi

echo "== a wrong sha256 fails makepkg =="
cp -R "$RECIPE" "$tmp/wrong"
cp "$INNER" "$tmp/wrong/"
python3 -I - "$tmp/wrong/PKGBUILD" "$sha256sums" <<'PY'
import sys
p, s = sys.argv[1:3]
t = open(p).read()
assert s in t
open(p, "w").write(t.replace(s, "0" * 64))
PY
rc=0
(cd "$tmp/wrong" && mp -f) > "$tmp/wrong.log" 2>&1 || rc=$?
if [ "$rc" != 0 ] && grep -qF 'FAILED' "$tmp/wrong.log" && grep -qF 'One or more files did not pass the validity check' "$tmp/wrong.log"; then
    pass "a wrong sum stops the build (makepkg exit $rc)"
else
    fail "a wrong sum did not stop the build" "exit $rc: $(tail -10 "$tmp/wrong.log")"
fi

echo "== install into a scratch root =="
R="$tmp/root"
mkdir -p "$R/var/lib/pacman" "$R/var/cache/pacman/pkg"
rc=0
fakeroot pacman -U --noconfirm --root "$R" --dbpath "$R/var/lib/pacman" --cachedir "$R/var/cache/pacman/pkg" \
    --nodeps --nodeps --noscriptlet --logfile "$tmp/pacman.log" --gpgdir /etc/pacman.d/gnupg "$PKG" > "$tmp/pacman.out" 2>&1 || rc=$?
if [ "$rc" = 0 ]; then pass "pacman -U into a scratch root"; else fail "pacman -U" "exit $rc: $(tail -10 "$tmp/pacman.out")"; fi
fakeroot pacman -Ql --root "$R" --dbpath "$R/var/lib/pacman" k2-bin > "$tmp/ql" 2>&1 && pass "pacman -Ql lists $(wc -l < "$tmp/ql") paths" || fail "pacman -Ql" "$(cat "$tmp/ql")"
out="$("$R/usr/bin/k2-daemon" --version 2>&1 || true)"
if grep -qF "$ver" <<<"$out"; then pass "the installed k2-daemon starts: $out"; else fail "k2-daemon --version" "$out"; fi
# The CLI and the daemon together, in a throwaway home: start the daemon by
# hand (no systemd user bus in a job) and ask the CLI who it is.
H="$tmp/home"
mkdir -p "$H"
env -i HOME="$H" PATH="$R/usr/bin:/usr/bin:/bin" "$R/usr/bin/k2-daemon" > "$tmp/daemon.log" 2>&1 &
DPID=$!
for _ in $(seq 120); do
    [ -s "$H/.k2/heartbeat.port" ] && [ -s "$H/.k2/heartbeat.token" ] && break
    sleep 0.5
done
boot=""
for _ in $(seq 120); do
    boot="$(curl -fsS --max-time 5 "http://127.0.0.1:$(cat "$H/.k2/heartbeat.port" 2>/dev/null)/boot-status" 2>/dev/null || true)"
    grep -q '"ready"' <<<"$boot" && break
    sleep 0.5
done
# `k2 whoami` needs a workspace session, so it can't stand in for "the CLI reaches
# the daemon" here. "cannot determine identity" is the daemon-backed path's answer.
rc=0
out="$(env -i HOME="$H" PATH="$R/usr/bin:/usr/bin:/bin" "$R/usr/bin/k2" whoami 2>&1)" || rc=$?
kill "$DPID" 2>/dev/null || true
wait "$DPID" 2>/dev/null || true
if grep -q '"ready"' <<<"$boot" && grep -qF "$ver" <<<"$boot"; then pass "the installed daemon comes up ready as $ver"; else fail "boot-status" "$boot; daemon log: $(tail -5 "$tmp/daemon.log")"; fi
if [ "$rc" = 1 ] && grep -qF 'cannot determine identity' <<<"$out"; then pass "the installed CLI runs and reaches its identity check"; else fail "k2 whoami exit $rc" "$out"; fi

echo "== omacom/omarchy-pkgs' own checks =="
REF="${K2_OMARCHY_PKGS_REF:-master}"
git clone -q --filter=blob:none https://github.com/omacom/omarchy-pkgs.git "$tmp/omarchy"
git -C "$tmp/omarchy" checkout -q "$REF"
echo "  omarchy-pkgs at $(git -C "$tmp/omarchy" rev-parse --short HEAD)"
cp -R "$RECIPE" "$tmp/omarchy/pkgbuilds/k2-bin"
for t in tests/pinned-sources.sh tests/pkgname-unique.sh tests/package-metadata.sh; do
    if (cd "$tmp/omarchy" && bash "$t") > "$tmp/t.out" 2>&1; then pass "their $t"; else fail "their $t" "$(tail -15 "$tmp/t.out")"; fi
done
if (cd "$tmp/omarchy" && python3 helpers/upstream-watch.py check pkgbuilds/k2-bin) > "$tmp/w.out" 2>&1; then
    pass "their upstream-watch check: $(tail -1 "$tmp/w.out")"
else
    fail "their upstream-watch check" "$(tail -15 "$tmp/w.out")"
fi
if command -v docker >/dev/null || command -v podman >/dev/null; then
    if (cd "$tmp/omarchy" && bin/build --package k2-bin) > "$tmp/ob.out" 2>&1; then
        pass "their bin/build --package k2-bin"
    else
        fail "their bin/build" "$(tail -25 "$tmp/ob.out")"
    fi
else
    echo "  NOT RUN: their bin/build (no Docker or Podman on $(uname -n)); makepkg above stands in for it"
fi

echo
echo "omarchy-recipe-build: $npass passed, $nfail failed"
[ "$nfail" -eq 0 ]
