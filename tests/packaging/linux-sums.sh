#!/usr/bin/env bash
# Tests for scripts/linux-sums.sh with a stub gh on PATH. The stub keeps
# each "release" as a folder of files, so uploads, clobbers and digests are
# real files. Nothing here talks to GitHub. No skips.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SUMS="$ROOT/scripts/linux-sums.sh"

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
export -f sha256_of

mkdir -p "$tmp/bin"
cat > "$tmp/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
echo "gh $*" >> "$STUB_CALLS"
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi; }
case "$1 $2" in
    "release view")
        tag="$3"; [ -d "$STUB_REL/$tag" ] || { echo "release not found" >&2; exit 1; }
        ls -1 "$STUB_REL/$tag"
        ;;
    "release download")
        tag="$3"; shift 3
        pattern=""; dir="."
        while [ $# -gt 0 ]; do
            case "$1" in
                --pattern) pattern="$2"; shift 2 ;;
                --dir) dir="$2"; shift 2 ;;
                *) shift ;;
            esac
        done
        [ -f "$STUB_REL/$tag/$pattern" ] || { echo "no asset $pattern" >&2; exit 1; }
        cp "$STUB_REL/$tag/$pattern" "$dir/$pattern"
        ;;
    "release upload")
        tag="$3"; file="$4"
        [ -d "$STUB_REL/$tag" ] || { echo "release not found" >&2; exit 1; }
        cp "$file" "$STUB_REL/$tag/$(basename "$file")"
        ;;
    "api repos/"*)
        tag="${2##*/}"
        for f in "$STUB_REL/$tag"/*; do
            n="$(basename "$f")"
            if [ -f "$STUB_REL/.digest-override/$n" ]; then
                d="$(cat "$STUB_REL/.digest-override/$n")"
            else
                d="sha256:$(sha "$f")"
            fi
            printf '%s\t%s\n' "$n" "$d"
        done
        ;;
    *)
        echo "gh stub: unexpected $*" >&2
        exit 1
        ;;
esac
STUB
chmod +x "$tmp/bin/gh"
export PATH="$tmp/bin:$PATH"
export K2_SUMS_POLL_SECS=0
export K2_RELEASE_REPO="stub/K2"

V=9.9.9
TAG="v$V"

# fresh_release <case> → exports STUB_REL/STUB_CALLS; sets REL to the tag folder.
fresh_release() {
    export STUB_REL="$tmp/$1/releases"
    export STUB_CALLS="$tmp/$1/calls"
    REL="$STUB_REL/$TAG"
    mkdir -p "$REL" "$STUB_REL/.digest-override"
    : > "$STUB_CALLS"
}

# app_linux_assets → writes the three app-linux packages into the release and
# their lines into $tmp/<case>/app.lines (the way app-linux.yml builds them).
app_linux_assets() {
    local lines="$1"
    : > "$lines"
    for n in "K2_${V}_amd64.AppImage" "K2_${V}_amd64.deb" "K2-${V}-1.x86_64.rpm"; do
        printf 'payload %s\n' "$n" > "$REL/$n"
        printf '%s  %s\n' "$(sha256_of "$REL/$n")" "$n" >> "$lines"
    done
}

arch_asset() {
    local lines="$1" n="k2-${V}-x86_64.pkg.tar.zst"
    printf 'arch payload %s\n' "${2:-1}" > "$tmp/$n"
    cp "$tmp/$n" "$REL/$n"
    "$SUMS" line "$tmp/$n" > "$lines"
}

H1="$(printf 'a%.0s' $(seq 1 64))"
H2="$(printf 'b%.0s' $(seq 1 64))"
H3="$(printf 'c%.0s' $(seq 1 64))"

echo "== merge-text =="
printf '%s  K2_1_amd64.deb\n%s  K2-1-1.x86_64.rpm\n' "$H1" "$H2" > "$tmp/old"
printf '%s  k2-1-x86_64.pkg.tar.zst\n' "$H3" > "$tmp/new"
out="$("$SUMS" merge-text "$tmp/old" "$tmp/new")"
expect="$(printf '%s  K2_1_amd64.deb\n%s  K2-1-1.x86_64.rpm\n%s  k2-1-x86_64.pkg.tar.zst' "$H1" "$H2" "$H3")"
[ "$out" = "$expect" ] && pass "append keeps existing order" || fail "append keeps existing order" "$out"

printf '%s  K2-1-1.x86_64.rpm\n' "$H3" > "$tmp/new"
out="$("$SUMS" merge-text "$tmp/old" "$tmp/new")"
expect="$(printf '%s  K2_1_amd64.deb\n%s  K2-1-1.x86_64.rpm' "$H1" "$H3")"
[ "$out" = "$expect" ] && pass "replace in place" || fail "replace in place" "$out"

printf '%s *K2-1-1.x86_64.rpm\n' "$H3" > "$tmp/new"
out="$("$SUMS" merge-text - "$tmp/new")"
[ "$out" = "$H3  K2-1-1.x86_64.rpm" ] && pass "binary-mode star normalized, no existing file" || fail "star normalize" "$out"

for bad in "$H1  ../evil.deb" "$H1  dir/evil.deb" "$H1  .hidden.deb" "$H1 single-space.deb" \
           "ABCDEF  upper.deb" "${H1}0  long.deb" "$H1  name with space.deb" "$H1  "; do
    printf '%s\n' "$bad" > "$tmp/new"
    if err="$("$SUMS" merge-text "$tmp/old" "$tmp/new" 2>&1)"; then
        fail "rejects '$bad'" "merge succeeded: $err"
    elif printf '%s' "$err" | grep -q "malformed new line"; then
        pass "rejects '$bad'"
    else
        fail "rejects '$bad'" "wrong error: $err"
    fi
done

printf '%s  x.deb\n%s  x.deb\n' "$H1" "$H2" > "$tmp/dupold"
printf '%s  y.deb\n' "$H3" > "$tmp/new"
if err="$("$SUMS" merge-text "$tmp/dupold" "$tmp/new" 2>&1)"; then
    fail "duplicate existing name fails" "succeeded"
else
    printf '%s' "$err" | grep -q "duplicate name in existing" && pass "duplicate existing name fails" || fail "duplicate existing name fails" "$err"
fi
: > "$tmp/empty"
if err="$("$SUMS" merge-text "$tmp/old" "$tmp/empty" 2>&1)"; then
    fail "empty new lines fail" "succeeded"
else
    pass "empty new lines fail"
fi

echo "== merge against a release =="
fresh_release arch-first
arch_asset "$tmp/arch.lines"
if err="$("$SUMS" merge "$TAG" "$tmp/arch.lines" 2>&1)"; then
    fail "arch merge with no SUMS fails loudly" "succeeded: $err"
elif printf '%s' "$err" | grep -q "has no SHA256SUMS-linux-x86_64.txt" \
     && ! grep -q "release upload" "$STUB_CALLS"; then
    pass "arch merge with no SUMS fails loudly, uploads nothing"
else
    fail "arch merge with no SUMS fails loudly" "$err / $(cat "$STUB_CALLS")"
fi
if err="$("$SUMS" wait "$TAG" 0 2>&1)"; then
    fail "wait times out without SUMS" "succeeded"
else
    printf '%s' "$err" | grep -q "still has no" && pass "wait times out without SUMS" || fail "wait times out" "$err"
fi

fresh_release normal
app_linux_assets "$tmp/app.lines"
"$SUMS" merge "$TAG" "$tmp/app.lines" --create-if-missing >/dev/null 2>"$tmp/err" \
    && pass "app-linux creates SUMS" || fail "app-linux creates SUMS" "$(cat "$tmp/err")"
"$SUMS" wait "$TAG" 0 2>/dev/null && pass "wait sees SUMS" || fail "wait sees SUMS" ""
arch_asset "$tmp/arch.lines"
"$SUMS" merge "$TAG" "$tmp/arch.lines" >/dev/null 2>"$tmp/err" || fail "arch merge" "$(cat "$tmp/err")"
expect="$(cat "$tmp/app.lines" "$tmp/arch.lines")"
[ "$(cat "$REL/SHA256SUMS-linux-x86_64.txt")" = "$expect" ] \
    && pass "arch line appended after the three app-linux lines" \
    || fail "arch line appended" "$(cat "$REL/SHA256SUMS-linux-x86_64.txt")"
"$SUMS" merge "$TAG" "$tmp/arch.lines" >/dev/null 2>&1
[ "$(cat "$REL/SHA256SUMS-linux-x86_64.txt")" = "$expect" ] \
    && pass "second arch merge is idempotent" || fail "idempotent" "$(cat "$REL/SHA256SUMS-linux-x86_64.txt")"
arch_asset "$tmp/arch.lines" rebuilt
"$SUMS" merge "$TAG" "$tmp/arch.lines" >/dev/null 2>&1
[ "$(grep -c 'pkg.tar.zst' "$REL/SHA256SUMS-linux-x86_64.txt")" = 1 ] \
    && grep -qxF "$(cat "$tmp/arch.lines")" "$REL/SHA256SUMS-linux-x86_64.txt" \
    && pass "rebuilt arch package replaces its line" || fail "rebuilt replaces" "$(cat "$REL/SHA256SUMS-linux-x86_64.txt")"
# app-linux re-run after arch: must keep the pkg line.
"$SUMS" merge "$TAG" "$tmp/app.lines" --create-if-missing >/dev/null 2>&1
grep -qxF "$(cat "$tmp/arch.lines")" "$REL/SHA256SUMS-linux-x86_64.txt" \
    && [ "$(grep -c . "$REL/SHA256SUMS-linux-x86_64.txt")" = 4 ] \
    && pass "app-linux re-run keeps the arch line" || fail "app-linux re-run keeps arch" "$(cat "$REL/SHA256SUMS-linux-x86_64.txt")"

echo "== check =="
"$SUMS" check "$V" 2>"$tmp/err" && pass "complete release passes" || fail "complete release passes" "$(cat "$tmp/err")"

fresh_release missing-line
app_linux_assets "$tmp/app.lines"
"$SUMS" merge "$TAG" "$tmp/app.lines" --create-if-missing >/dev/null 2>&1
arch_asset "$tmp/arch.lines"   # asset up, line never merged (the v0.46.0 bug)
if err="$("$SUMS" check "$V" 2>&1)"; then
    fail "pkg without a SUMS line fails" "passed"
else
    printf '%s' "$err" | grep -q "no SHA256SUMS-linux-x86_64.txt line for k2-${V}-x86_64.pkg.tar.zst" \
        && pass "pkg without a SUMS line fails, naming it" || fail "pkg without line" "$err"
fi

fresh_release missing-asset
app_linux_assets "$tmp/app.lines"
"$SUMS" merge "$TAG" "$tmp/app.lines" --create-if-missing >/dev/null 2>&1
if err="$("$SUMS" check "$V" 2>&1)"; then
    fail "missing arch asset fails" "passed"
else
    printf '%s' "$err" | grep -q "not on the release yet: k2-${V}-x86_64.pkg.tar.zst" \
        && pass "missing arch asset fails, naming it" || fail "missing arch asset" "$err"
fi

fresh_release wrong-hash
app_linux_assets "$tmp/app.lines"
arch_asset "$tmp/arch.lines"
cat "$tmp/app.lines" "$tmp/arch.lines" > "$tmp/all.lines"
"$SUMS" merge "$TAG" "$tmp/all.lines" --create-if-missing >/dev/null 2>&1
printf 'sha256:%s\n' "$H1" > "$STUB_REL/.digest-override/K2_${V}_amd64.deb"
if err="$("$SUMS" check "$V" --wait-min 5 2>&1)"; then
    fail "digest mismatch fails" "passed"
else
    printf '%s' "$err" | grep -q "K2_${V}_amd64.deb: SUMS says" && printf '%s' "$err" | grep -q "is wrong" \
        && pass "digest mismatch fails at once (no wait)" || fail "digest mismatch" "$err"
fi

fresh_release extra-linux-asset
app_linux_assets "$tmp/app.lines"
arch_asset "$tmp/arch.lines"
cat "$tmp/app.lines" "$tmp/arch.lines" > "$tmp/all.lines"
"$SUMS" merge "$TAG" "$tmp/all.lines" --create-if-missing >/dev/null 2>&1
printf 'x\n' > "$REL/k2-daemon_${V}_amd64.deb"
if err="$("$SUMS" check "$V" 2>&1)"; then
    fail "any other Linux package also needs a line" "passed"
else
    printf '%s' "$err" | grep -q "no SHA256SUMS-linux-x86_64.txt line for k2-daemon_${V}_amd64.deb" \
        && pass "any other Linux package also needs a line" || fail "extra asset" "$err"
fi

echo ""
echo "linux-sums: ${pass} passed, ${fail} failed"
[ "$fail" = 0 ]
