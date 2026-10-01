#!/usr/bin/env bash
# Tests for scripts/check-arch-pkgbuild.sh. Each case copies the real
# packaging/arch tree into a temp root, breaks one thing, and asserts the
# check fails with a message naming it. The unbroken copy must pass.
# Runs on the Mac and on ubuntu-latest. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CHECK="$ROOT/scripts/check-arch-pkgbuild.sh"

pass=0
fail=0
pass() { echo "  PASS: $1"; pass=$((pass + 1)); }
fail() { echo "  FAIL: $1" >&2; echo "        $2" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# fixture <name> → prints a fresh temp repo root with a copy of the files
# the check reads.
fixture() {
    local d="$tmp/$1"
    mkdir -p "$d/packaging" "$d/src-tauri"
    cp -R "$ROOT/packaging/arch" "$d/packaging/arch"
    cp "$ROOT/src-tauri/tauri.conf.json" "$d/src-tauri/tauri.conf.json"
    printf '%s\n' "$d"
}

# Replace the first sha256sums entry with $1 (perl: same on GNU and BSD).
set_first_sum() {
    local pkgbuild="$1" sum="$2"
    SUM="$sum" perl -0pi -e 's/(sha256sums=\(\n\s*)\x27[^\x27]*\x27/$1\x27$ENV{SUM}\x27/' "$pkgbuild"
}

expect_pass() {
    local label="$1" root="$2" rc=0
    "$CHECK" "$root" >"$tmp/out" 2>"$tmp/err" || rc=$?
    if [ "$rc" -eq 0 ]; then
        pass "$label"
    else
        fail "$label" "rc=$rc err=$(cat "$tmp/err")"
    fi
}

expect_fail() {
    local label="$1" root="$2" needle="$3" rc=0
    "$CHECK" "$root" >"$tmp/out" 2>"$tmp/err" || rc=$?
    if [ "$rc" -ne 0 ] && grep -Fq -- "$needle" "$tmp/err"; then
        pass "$label"
    else
        fail "$label" "rc=$rc, wanted stderr to contain '$needle'; err=$(cat "$tmp/err")"
    fi
}

echo "== check-arch-pkgbuild.sh =="

expect_pass "real tree passes" "$ROOT"
r="$(fixture clean)"
expect_pass "unbroken copy passes" "$r"

# Count mismatch: drop one sum line.
r="$(fixture count)"
perl -0pi -e 's/(sha256sums=\(\n\s*\x27[^\x27]*\x27\n)\s*\x27[^\x27]*\x27\n/$1/' "$r/packaging/arch/PKGBUILD"
expect_fail "sum count mismatch fails" "$r" "sha256sums=() has 3"

# Missing local source.
r="$(fixture missing)"
rm "$r/packaging/arch/k2.desktop"
expect_fail "missing local source fails" "$r" "local source 'k2.desktop' is missing"

# The 0.43.0 break: a pinned sum that no longer matches the file.
r="$(fixture stale)"
set_first_sum "$r/packaging/arch/PKGBUILD" "6bb7459f3123a86e64a2ece31c99cc20381312c97e463c8de500e5425fd207a3"
printf '# edited after the sum was pinned\n' >> "$r/packaging/arch/k2-daemon.service"
expect_fail "stale sha256 on a local source fails" "$r" "local source 'k2-daemon.service' sha256 is"

# A correct pinned sum still passes.
r="$(fixture pinned)"
set_first_sum "$r/packaging/arch/PKGBUILD" "$(sha256_of "$r/packaging/arch/k2-daemon.service")"
expect_pass "correct pinned sha256 passes" "$r"

# A URL source must not use SKIP.
r="$(fixture url)"
perl -0pi -e 's/source=\(\n/source=(\n    \x27frpc.tar.gz::https:\/\/example.invalid\/frpc.tar.gz\x27\n/; s/sha256sums=\(\n/sha256sums=(\n    \x27SKIP\x27\n/' "$r/packaging/arch/PKGBUILD"
expect_fail "URL source with SKIP fails" "$r" "uses SKIP; downloads need a real sha256"

# Stale pkgver with no rewrite in build-release-pkg.sh.
r="$(fixture pkgver)"
perl -pi -e 's/^pkgver=.*/pkgver=0.0.1/' "$r/packaging/arch/PKGBUILD"
perl -pi -e 's/^(\s*)sed -i "s\/\^pkgver=/$1: sed -i "s\/^pkgver=/' "$r/packaging/arch/build-release-pkg.sh"
expect_fail "stale pkgver without a rewrite fails" "$r" "does not rewrite pkgver"

# Same stale pkgver passes while the build script rewrites it.
r="$(fixture pkgver-ok)"
perl -pi -e 's/^pkgver=.*/pkgver=0.0.1/' "$r/packaging/arch/PKGBUILD"
expect_pass "stale pkgver with the rewrite passes" "$r"

# Syntax errors.
r="$(fixture install-syntax)"
printf 'post_install() {\n' >> "$r/packaging/arch/k2.install"
expect_fail "k2.install syntax error fails" "$r" "bash -n k2.install"

r="$(fixture build-syntax)"
printf 'if then\n' >> "$r/packaging/arch/build-release-pkg.sh"
expect_fail "build-release-pkg.sh syntax error fails" "$r" "bash -n build-release-pkg.sh"

r="$(fixture install-missing)"
rm "$r/packaging/arch/k2.install"
expect_fail "missing install file fails" "$r" "install=k2.install is missing"

echo
if [ "$fail" -ne 0 ]; then
    echo "FAIL: $fail  PASS: $pass" >&2
    exit 1
fi
echo "PASS: $pass"
