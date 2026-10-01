#!/usr/bin/env bash
# check-arch-pkgbuild.sh — catch an Arch PKGBUILD break on every push,
# without Arch. Runs on GitHub-hosted ubuntu-latest (checks.yml) and on
# the Mac. Needs bash and sha256sum or shasum. No makepkg, no pacman.
#
# Why: the Arch package is only built by arch-package.yml on the k2-arch
# runner, after the tag is live. 0.43.0's tag build died in 14 seconds on
# a stale k2.install sha256. That is a text check; it belongs on every push.
#
# Checks:
#   - source=() and sha256sums=() have the same count;
#   - every local source (no "://") exists next to the PKGBUILD;
#   - every non-SKIP sum matches its local file;
#   - URL sources do not use SKIP (downloads keep a real sum);
#   - install= names a file that exists;
#   - pkgver equals src-tauri/tauri.conf.json, or build-release-pkg.sh
#     rewrites pkgver before makepkg (it does today: the release build
#     passes the tree version through, so the committed pkgver is stale);
#   - bash -n on PKGBUILD, the install file and build-release-pkg.sh.
#
# Usage: scripts/check-arch-pkgbuild.sh [repo-root]
#   repo-root defaults to this script's repo. The tests point it at a
#   temp copy with a deliberately broken PKGBUILD.
# Exit 0 when every check passes, 1 otherwise. No skips.

set -euo pipefail

ROOT="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
ROOT="$(cd "$ROOT" && pwd)"
ARCH_DIR="$ROOT/packaging/arch"
PKGBUILD="$ARCH_DIR/PKGBUILD"
BUILD_SCRIPT="$ARCH_DIR/build-release-pkg.sh"
TAURI_CONF="$ROOT/src-tauri/tauri.conf.json"

fails=0
ok() { echo "  ok:   $1"; }
bad() { echo "  FAIL: $1" >&2; fails=$((fails + 1)); }

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        echo "FATAL: need sha256sum or shasum" >&2
        exit 1
    fi
}

echo "== Arch PKGBUILD check ($PKGBUILD) =="

for f in "$PKGBUILD" "$BUILD_SCRIPT" "$TAURI_CONF"; do
    if [ ! -f "$f" ]; then
        echo "FATAL: missing $f" >&2
        exit 1
    fi
done

if bash -n "$PKGBUILD"; then ok "bash -n PKGBUILD"; else bad "bash -n PKGBUILD"; fi
if bash -n "$BUILD_SCRIPT"; then ok "bash -n build-release-pkg.sh"; else bad "bash -n build-release-pkg.sh"; fi

# Source the PKGBUILD in a clean bash with no inherited env. It only
# assigns variables and defines functions at top level, so nothing runs.
# Print one record per line: kind<TAB>index<TAB>value.
# shellcheck disable=SC2016 # the -c body expands in the child bash
dump="$(env -i PATH="/usr/bin:/bin" bash --noprofile --norc -c '
    set -eu
    cd "$(dirname "$1")"
    # shellcheck disable=SC1090
    source "$1"
    printf "pkgver\t0\t%s\n" "${pkgver:-}"
    printf "install\t0\t%s\n" "${install:-}"
    if ! declare -p source >/dev/null 2>&1; then echo "FATAL: no source=() array" >&2; exit 1; fi
    if ! declare -p sha256sums >/dev/null 2>&1; then echo "FATAL: no sha256sums=() array" >&2; exit 1; fi
    for i in "${!source[@]}"; do printf "source\t%s\t%s\n" "$i" "${source[$i]}"; done
    for i in "${!sha256sums[@]}"; do printf "sha256\t%s\t%s\n" "$i" "${sha256sums[$i]}"; done
    for arr in md5sums sha1sums sha224sums sha384sums sha512sums b2sums cksums; do
        if declare -p "$arr" >/dev/null 2>&1; then
            eval "n=\${#${arr}[@]}"
            printf "othersum\t%s\t%s\n" "$n" "$arr"
        fi
    done
' _ "$PKGBUILD")" || { echo "FATAL: could not source $PKGBUILD" >&2; exit 1; }

sources=()
sums=()
pkgver=""
install_file=""
others=()
while IFS=$'\t' read -r kind idx value; do
    case "$kind" in
        pkgver) pkgver="$value" ;;
        install) install_file="$value" ;;
        source) sources[idx]="$value" ;;
        sha256) sums[idx]="$value" ;;
        othersum) others+=("${value}:${idx}") ;;
        *) echo "FATAL: unexpected record '$kind'" >&2; exit 1 ;;
    esac
done <<EOF
$dump
EOF

n_src="${#sources[@]}"
n_sum="${#sums[@]}"
if [ "$n_src" -eq 0 ]; then
    bad "source=() is empty"
elif [ "$n_src" -eq "$n_sum" ]; then
    ok "source=() and sha256sums=() both have $n_src entries"
else
    bad "source=() has $n_src entries but sha256sums=() has $n_sum"
fi
for o in ${others[@]+"${others[@]}"}; do
    name="${o%%:*}"
    count="${o##*:}"
    if [ "$count" -ne "$n_src" ]; then
        bad "$name=() has $count entries but source=() has $n_src"
    fi
done

i=0
while [ "$i" -lt "$n_src" ]; do
    entry="${sources[$i]}"
    sum="${sums[$i]:-}"
    # makepkg accepts "name::location"; the location decides local vs URL.
    location="${entry#*::}"
    case "$location" in
        *://*)
            if [ "$sum" = "SKIP" ]; then
                bad "URL source '$entry' uses SKIP; downloads need a real sha256"
            elif [[ "$sum" =~ ^[0-9a-f]{64}$ ]]; then
                ok "URL source '$entry' has a sha256"
            else
                bad "URL source '$entry' has a malformed sha256 '$sum'"
            fi
            ;;
        *)
            file="$ARCH_DIR/$location"
            if [ ! -f "$file" ]; then
                bad "local source '$entry' is missing (expected $file)"
            elif [ "$sum" = "SKIP" ]; then
                ok "local source '$entry' exists (SKIP)"
            elif [ -z "$sum" ]; then
                bad "local source '$entry' has no sha256 entry"
            else
                got="$(sha256_of "$file")"
                if [ "$got" = "$sum" ]; then
                    ok "local source '$entry' matches its sha256"
                else
                    bad "local source '$entry' sha256 is $got but PKGBUILD says $sum (use 'SKIP' for in-repo files)"
                fi
            fi
            ;;
    esac
    i=$((i + 1))
done

if [ -n "$install_file" ]; then
    if [ -f "$ARCH_DIR/$install_file" ]; then
        if bash -n "$ARCH_DIR/$install_file"; then
            ok "bash -n $install_file"
        else
            bad "bash -n $install_file"
        fi
    else
        bad "install=$install_file is missing (expected $ARCH_DIR/$install_file)"
    fi
fi

conf_ver="$(sed -nE 's/.*"version"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/p' "$TAURI_CONF" | head -1)"
# The pkgver rewrite line in build-release-pkg.sh, matched as text.
# shellcheck disable=SC2016
rewrite_re='^[[:space:]]*sed -i "s/\^pkgver=\.\*/pkgver=\$\{version\}/" "\$pkgbuild"'
if [ -z "$conf_ver" ]; then
    bad "no version in $TAURI_CONF"
elif [ -z "$pkgver" ]; then
    bad "PKGBUILD has no pkgver"
elif [ "$pkgver" = "$conf_ver" ]; then
    ok "pkgver $pkgver matches tauri.conf.json"
elif grep -Eq "$rewrite_re" "$BUILD_SCRIPT"; then
    ok "pkgver $pkgver differs from tauri.conf.json $conf_ver; build-release-pkg.sh rewrites it before makepkg"
else
    bad "pkgver $pkgver != tauri.conf.json $conf_ver and build-release-pkg.sh does not rewrite pkgver"
fi

echo
if [ "$fails" -ne 0 ]; then
    echo "FAIL: $fails Arch PKGBUILD check(s) failed" >&2
    exit 1
fi
echo "PASS: Arch PKGBUILD check"
