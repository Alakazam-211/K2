#!/usr/bin/env bash
# scripts/lib/release-source-archives.sh with a stub curl: the archives are
# fetched and kept only when their sha256 is the pin; a wrong byte or a
# failed download stops with no file left. No network; temp dirs only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
pass=0
fail=0
ok() { echo "  PASS: $1"; pass=$((pass + 1)); }
no() { echo "  FAIL: $1" >&2; fail=$((fail + 1)); }

# Stub curl: `-o <file> <url>` writes "bytes of <url>", unless the URL ends
# in /down (fails like curl -f does).
mkdir -p "$WORK/bin"
cat > "$WORK/bin/curl" <<'EOF'
#!/usr/bin/env bash
out="" url=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o) out="$2"; shift 2 ;;
        --retry|--retry-delay) shift 2 ;;
        -*) shift ;;
        *) url="$1"; shift ;;
    esac
done
case "$url" in */down) echo "curl: (22) 404" >&2; exit 22 ;; esac
printf 'bytes of %s' "$url" > "$out"
EOF
chmod +x "$WORK/bin/curl"
export PATH="$WORK/bin:$PATH"

sha() { printf '%s' "$1" | { shasum -a 256 2>/dev/null || sha256sum; } | awk '{print $1}'; }

# shellcheck source=scripts/lib/release-source-archives.sh
source "$ROOT/scripts/lib/release-source-archives.sh"

echo "── release source archives ──"
# The real pins: two archives, 64-hex sha256s, the libgit2 tag and crate.
[ "${#K2_SOURCE_ARCHIVES[@]}" -eq 2 ] && ok "two source archives are pinned" || no "two source archives are pinned"
for e in "${K2_SOURCE_ARCHIVES[@]}"; do
    IFS='|' read -r n u s l <<<"$e"
    [[ "$s" =~ ^[0-9a-f]{64}$ ]] && ok "$n: its pin is a sha256" || no "$n: its pin is a sha256"
    [[ "$u" == https://* ]] && ok "$n: fetched over https" || no "$n: fetched over https"
done
grep -qF "https://github.com/libgit2/libgit2/archive/refs/tags/v1.8.1.tar.gz" "$ROOT/scripts/lib/release-source-archives.sh" \
    && ok "libgit2 v1.8.1's tarball" || no "libgit2 v1.8.1's tarball"
grep -qF "libgit2-sys-0.17.0%2B1.8.1.crate|10472326a8a6477c3c20a64547b0059e4b0d086869eee31e6d7da728a8eb7224" "$ROOT/scripts/lib/release-source-archives.sh" \
    && ok "the .crate's pin is the checksum earlier releases' Cargo.lock pinned" || no "the .crate's pin is the checksum earlier releases' Cargo.lock pinned"
grep -qF 'source "$PROJECT_DIR/scripts/lib/release-source-archives.sh"' "$ROOT/scripts/release.sh" \
    && grep -qF 'k2_fetch_source_archives "$DIST_DIR/source-archives"' "$ROOT/scripts/release.sh" \
    && ok "release.sh Step 9a attaches them" || no "release.sh Step 9a attaches them"

# Stand-in pins for the stub's bytes.
K2_SOURCE_ARCHIVES=(
    "a.tar.gz|https://example.invalid/a|$(sha 'bytes of https://example.invalid/a')|A source"
    "b.crate|https://example.invalid/b|$(sha 'bytes of https://example.invalid/b')|B source"
)
out="$(k2_fetch_source_archives "$WORK/d1")"
[ "$out" = "$(printf '%s\n%s' "$WORK/d1/a.tar.gz#A source" "$WORK/d1/b.crate#B source")" ] \
    && ok "prints each archive with its label" || no "prints each archive with its label (got: $out)"
[ "$(cat "$WORK/d1/b.crate")" = "bytes of https://example.invalid/b" ] && ok "keeps the checked bytes" || no "keeps the checked bytes"

K2_SOURCE_ARCHIVES[1]="b.crate|https://example.invalid/b|$(sha 'other bytes')|B source"
if k2_fetch_source_archives "$WORK/d2" >/dev/null 2>"$WORK/err"; then
    no "a sha256 mismatch stops it"
else
    grep -qF "the pin in scripts/lib/release-source-archives.sh says" "$WORK/err" && ok "a sha256 mismatch stops it, naming the pin" || no "a sha256 mismatch names the pin"
    [ ! -e "$WORK/d2/b.crate" ] && [ ! -e "$WORK/d2/b.crate.part" ] && ok "no mismatched file is left" || no "no mismatched file is left"
fi

K2_SOURCE_ARCHIVES[1]="b.crate|https://example.invalid/down|$(sha 'x')|B source"
if k2_fetch_source_archives "$WORK/d3" >/dev/null 2>"$WORK/err"; then
    no "a failed download stops it"
else
    grep -qF "could not download b.crate" "$WORK/err" && ok "a failed download stops it" || no "a failed download says which"
fi

echo ""
echo "release source archives: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
