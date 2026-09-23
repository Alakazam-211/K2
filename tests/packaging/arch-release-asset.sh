#!/usr/bin/env bash
# release.sh must FATAL when the Arch package is missing, and name
# k2-<version>-x86_64.pkg.tar.zst when describing the asset.
# Does not invoke gh. No skip-if-missing.

set -euo pipefail

# This test resolves the tree version itself. An inherited VERSION would
# override tauri.conf.json inside k2_arch_resolve_version.
unset VERSION || true

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

# shellcheck source=../../scripts/arch-release-asset.sh
source "$ROOT/scripts/arch-release-asset.sh"
# shellcheck source=../../packaging/arch/build-release-pkg.sh
source "$ROOT/packaging/arch/build-release-pkg.sh"

echo "== Arch release asset gate =="

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

ver="1.2.3"
name="k2-${ver}-x86_64.pkg.tar.zst"

rc=0
out="$(k2_require_arch_pkg_asset "$tmp" "$ver" 2>"$tmp/missing.err")" || rc=$?
if [ "$rc" -ne 0 ] && grep -q 'FATAL' "$tmp/missing.err" && grep -F -q "$name" "$tmp/missing.err"; then
    pass "missing package is FATAL and names ${name}"
else
    fail "missing package is FATAL and names ${name}" "rc=${rc} err=$(cat "$tmp/missing.err" 2>/dev/null || true) out=${out:-}"
fi
if [ -n "${out:-}" ]; then
    fail "missing package prints no asset path" "stdout was: ${out}"
else
    pass "missing package prints no asset path"
fi

# A .sig alone is not the package (pacman is the install path; no .sig required).
: > "$tmp/${name}.sig"
rc=0
out="$(k2_require_arch_pkg_asset "$tmp" "$ver" 2>"$tmp/sigonly.err")" || rc=$?
if [ "$rc" -ne 0 ] && grep -q 'FATAL' "$tmp/sigonly.err"; then
    pass "sig-only is still FATAL"
else
    fail "sig-only is still FATAL" "rc=${rc}"
fi
rm -f "$tmp/${name}.sig"

printf 'not-a-real-pkg\n' > "$tmp/$name"
rc=0
out="$(k2_require_arch_pkg_asset "$tmp" "$ver" 2>"$tmp/present.err")" || rc=$?
if [ "$rc" -eq 0 ] && [ "$out" = "$tmp/$name" ]; then
    pass "present package asset path includes ${name}"
else
    fail "present package asset path includes ${name}" "rc=${rc} out=${out:-}"
fi
case "$out" in
    */"$name") pass "asset description ends with ${name}" ;;
    *) fail "asset description ends with ${name}" "out=${out:-}" ;;
esac

rc=0
out="$(k2_require_arch_pkg_asset "$tmp" "v${ver}" 2>"$tmp/vprefix.err")" || rc=$?
if [ "$rc" -ne 0 ] && grep -q 'FATAL' "$tmp/vprefix.err" && grep -F -q "k2-v${ver}-x86_64.pkg.tar.zst" "$tmp/vprefix.err"; then
    pass "v prefix is FATAL"
else
    fail "v prefix is FATAL" "rc=${rc}"
fi

# release.sh attaches a staged package and does not stop the tag when it
# is absent. arch-package.yml uploads after the tag. No .sig, no gh here.
rel="$ROOT/scripts/release.sh"
call_line="$(grep -n 'ARCH_PKG_NAME=' "$rel" | head -1 | cut -d: -f1 || true)"
gh_line="$(grep -nE '^gh release create ' "$rel" | head -1 | cut -d: -f1 || true)"
asset_line="$(grep -n 'ASSETS+=("\$DIST_DIR/\$ARCH_PKG_NAME")' "$rel" | head -1 | cut -d: -f1 || true)"
if [ -n "$call_line" ] && [ -n "$gh_line" ] && [ -n "$asset_line" ] \
    && [ "$call_line" -lt "$asset_line" ] && [ "$asset_line" -lt "$gh_line" ]; then
    pass "release.sh attaches a staged Arch package before gh release create"
else
    fail "release.sh attaches a staged Arch package before gh release create" "call=${call_line:-none} asset=${asset_line:-none} gh=${gh_line:-none}"
fi
if grep -F -q 'k2_require_arch_pkg_asset "$DIST_DIR" "$VERSION")" || exit 1' "$rel"; then
    fail "release.sh does not fatal when the Arch package is absent" "still exits on k2_require_arch_pkg_asset"
else
    pass "release.sh does not fatal when the Arch package is absent"
fi
if grep -F -q 'arch-package.yml on the k2-arch runner uploads it after the tag' "$rel"; then
    pass "release.sh points a missing package at the k2-arch tag job"
else
    fail "release.sh points a missing package at the k2-arch tag job" "warning text missing"
fi
if grep -F -q 'k2-${VERSION}-x86_64.pkg.tar.zst' "$rel"; then
    pass "release.sh names k2-\${VERSION}-x86_64.pkg.tar.zst"
else
    fail "release.sh names k2-\${VERSION}-x86_64.pkg.tar.zst" "filename not in release.sh"
fi

block="$(sed -n "${call_line},$((asset_line + 6))p" "$rel")"
if printf '%s\n' "$block" | grep -F -q '.sig'; then
    fail "Arch asset attach does not require a .sig" "sig mentioned in attach block"
else
    pass "Arch asset attach does not require a .sig"
fi
if printf '%s\n' "$block" | grep -F -q 'if [ -s "$DIST_DIR/$ARCH_PKG_NAME" ]'; then
    pass "Arch asset attach skips a missing file"
else
    fail "Arch asset attach skips a missing file" "size check not in attach block"
fi
if grep -F -q 'pkg.tar.zst' "$rel" && grep -n 'pkg.tar.zst' "$rel" | grep -E -q 'latest\.json|daemon-latest'; then
    fail "Arch package is not a latest.json key" "pkg.tar.zst shares a line with a latest.json name"
else
    pass "Arch package is not a latest.json key"
fi

# Build script: Arch-only, version from the tree, no install, no hardcoded pkgver.
build="$ROOT/packaging/arch/build-release-pkg.sh"
if grep -F -q '0.40.143' "$build"; then
    fail "build script does not hardcode 0.40.143" "stale version in $build"
else
    pass "build script does not hardcode 0.40.143"
fi
# Comments may say "do not pacman -U". Code lines must not install.
if grep -vE '^\s*(#|$)' "$build" | grep -F -q 'pacman -U' \
    || grep -vE '^\s*(#|$)' "$build" | grep -E -q 'makepkg[[:space:]]+(-[a-zA-Z]*i|--install)'; then
    fail "build script does not install the package" "pacman -U or makepkg -i in code"
else
    pass "build script does not install the package"
fi
if grep -F -q 'require-mail-oauth-build-env.sh' "$build" && grep -F -q '.env' "$build"; then
    pass "build script sources .env and the OAuth gate"
else
    fail "build script sources .env and the OAuth gate" "missing .env or require-mail-oauth-build-env.sh"
fi
if grep -F -q 'The Mac does not compile the GUI' "$ROOT/packaging/arch/README.md" \
    && grep -F -q 'pkgs.omarchy.org' "$ROOT/packaging/arch/README.md" \
    && grep -F -q 'dist/k2-$VERSION-x86_64.pkg.tar.zst' "$ROOT/packaging/arch/README.md"; then
    pass "README tells the Arch host to stage the package for release.sh"
else
    fail "README tells the Arch host to stage the package for release.sh" "missing release paragraph"
fi

conf_ver="$(sed -nE 's/.*"version"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/p' "$ROOT/src-tauri/tauri.conf.json" | head -1)"
got="$(k2_arch_resolve_version "$ROOT" "")"
if [ -n "$conf_ver" ] && [ "$got" = "$conf_ver" ]; then
    pass "version resolves from tauri.conf.json (${got})"
else
    fail "version resolves from tauri.conf.json" "got=${got:-empty} conf=${conf_ver:-empty}"
fi
got="$(VERSION=9.8.7 k2_arch_resolve_version "$ROOT" "")"
if [ "$got" = "9.8.7" ]; then
    pass "VERSION environment overrides the tree"
else
    fail "VERSION environment overrides the tree" "got=${got:-empty}"
fi
rc=0
k2_arch_resolve_version "$ROOT" "v9.8.7" >/dev/null 2>"$tmp/badver.err" || rc=$?
if [ "$rc" -ne 0 ] && grep -q 'FATAL' "$tmp/badver.err"; then
    pass "resolver rejects a v prefix"
else
    fail "resolver rejects a v prefix" "rc=${rc}"
fi
rc=0
VERSION=1.2.3 k2_arch_resolve_version "$ROOT" "9.9.9" >/dev/null 2>"$tmp/disagree.err" || rc=$?
if [ "$rc" -ne 0 ] && grep -q 'FATAL' "$tmp/disagree.err"; then
    pass "argument and environment disagreement is FATAL"
else
    fail "argument and environment disagreement is FATAL" "rc=${rc}"
fi

# Executing the build script on this Mac must fail before it touches PKGBUILD.
pkg_before="$(grep -E '^pkgver=' "$ROOT/packaging/arch/PKGBUILD" | head -1)"
set +e
bash "$build" >"$tmp/build.out" 2>"$tmp/build.err"
build_rc=$?
set -e
pkg_after="$(grep -E '^pkgver=' "$ROOT/packaging/arch/PKGBUILD" | head -1)"
if [ "$build_rc" -ne 0 ] && grep -q 'FATAL' "$tmp/build.err" && grep -q 'pacman' "$tmp/build.err"; then
    pass "build script FATALs when pacman is absent"
else
    fail "build script FATALs when pacman is absent" "rc=${build_rc}"
fi
if [ "$pkg_before" = "$pkg_after" ]; then
    pass "non-Arch run does not rewrite PKGBUILD pkgver"
else
    fail "non-Arch run does not rewrite PKGBUILD pkgver" "before=${pkg_before} after=${pkg_after}"
fi
if grep -E -q 'K2_GMAIL_CLIENT_(ID|SECRET)=[A-Za-z0-9._-]{8,}' "$tmp/build.out" "$tmp/build.err"; then
    fail "build script does not print OAuth secrets" "secret-like assignment in output"
else
    pass "build script does not print OAuth secrets"
fi

echo
if [ "$fail" -ne 0 ]; then
    echo "FAIL: $fail  PASS: $pass" >&2
    exit 1
fi
echo "PASS: $pass"
exit 0
