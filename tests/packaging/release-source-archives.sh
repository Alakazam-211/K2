#!/usr/bin/env bash
# scripts/lib/release-source-archives.sh with a stub curl and a local
# stand-in for the libgit2 repository: the tag archive is made only when the
# tag is the pinned commit and its tar has the pinned sha256; the .crate is
# kept only at its pinned sha256; any miss stops with no file left. No
# network; temp dirs only.
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

sha() { { shasum -a 256 2>/dev/null || sha256sum; } | awk '{print $1}'; }

# shellcheck source=scripts/lib/release-source-archives.sh
source "$ROOT/scripts/lib/release-source-archives.sh"
LIB="$ROOT/scripts/lib/release-source-archives.sh"

echo "── release source archives ──"
# The real pins.
[ "$K2_LIBGIT2_REPO" = "https://github.com/libgit2/libgit2.git" ] && [ "$K2_LIBGIT2_TAG" = v1.8.1 ] \
    && ok "libgit2's repository and tag v1.8.1" || no "libgit2's repository and tag v1.8.1"
[[ "$K2_LIBGIT2_COMMIT" =~ ^[0-9a-f]{40}$ ]] && ok "the tag's commit is pinned" || no "the tag's commit is pinned"
[[ "$K2_LIBGIT2_TAR_SHA256" =~ ^[0-9a-f]{64}$ ]] && ok "the tar's sha256 is pinned" || no "the tar's sha256 is pinned"
[ "$K2_LIBGIT2_ASSET" = libgit2-1.8.1-source.tar.gz ] && ok "attached as libgit2-1.8.1-source.tar.gz" || no "attached as libgit2-1.8.1-source.tar.gz"
grep -qF "libgit2-sys-0.17.0%2B1.8.1.crate|10472326a8a6477c3c20a64547b0059e4b0d086869eee31e6d7da728a8eb7224" "$LIB" \
    && ok "the .crate's pin is the checksum earlier releases' Cargo.lock pinned" || no "the .crate's pin is the checksum earlier releases' Cargo.lock pinned"
! grep -qF "github.com/libgit2/libgit2/archive" "$LIB" && ok "no GitHub generated archive is pinned" || no "no GitHub generated archive is pinned"
grep -qF 'source "$PROJECT_DIR/scripts/lib/release-source-archives.sh"' "$ROOT/scripts/release.sh" \
    && grep -qF 'k2_fetch_source_archives "$DIST_DIR/source-archives"' "$ROOT/scripts/release.sh" \
    && ok "release.sh Step 9a attaches them" || no "release.sh Step 9a attaches them"

# A stand-in libgit2: a repository with a tag, and stand-in pins.
up="$WORK/upstream"
git init -q "$up"
printf 'stand-in source\n' > "$up/README"
git -C "$up" add README
git -C "$up" -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false commit -q -m one
git -C "$up" tag v1.8.1
K2_LIBGIT2_REPO="$up"
K2_LIBGIT2_COMMIT="$(git -C "$up" rev-parse HEAD)"
K2_LIBGIT2_TAR_SHA256="$(git -C "$up" archive --format=tar --prefix=libgit2-1.8.1/ HEAD | sha)"
K2_SOURCE_ARCHIVES=("b.crate|https://example.invalid/b|$(printf 'bytes of https://example.invalid/b' | sha)|B source")
reset_pins() { K2_LIBGIT2_COMMIT="$(git -C "$up" rev-parse HEAD)"; K2_LIBGIT2_TAR_SHA256="$(git -C "$up" archive --format=tar --prefix=libgit2-1.8.1/ HEAD | sha)"; }

out="$(k2_fetch_source_archives "$WORK/d1")"
[ "$out" = "$(printf '%s\n%s' "$WORK/d1/libgit2-1.8.1-source.tar.gz#$K2_LIBGIT2_LABEL" "$WORK/d1/b.crate#B source")" ] \
    && ok "prints each archive with its label" || no "prints each archive with its label (got: $out)"
[ "$(gzip -dc "$WORK/d1/libgit2-1.8.1-source.tar.gz" | sha)" = "$K2_LIBGIT2_TAR_SHA256" ] \
    && ok "the tag archive is the checked tar, gzipped" || no "the tag archive is the checked tar, gzipped"
gzip -dc "$WORK/d1/libgit2-1.8.1-source.tar.gz" | tar -tf - | grep -qx 'libgit2-1.8.1/README' \
    && ok "its files sit under libgit2-1.8.1/" || no "its files sit under libgit2-1.8.1/"
[ ! -e "$WORK/d1/libgit2-1.8.1-source.tar.gz.tar" ] && ok "no uncompressed tar is left" || no "no uncompressed tar is left"
k2_fetch_source_archives "$WORK/d1b" >/dev/null
cmp -s "$WORK/d1/libgit2-1.8.1-source.tar.gz" "$WORK/d1b/libgit2-1.8.1-source.tar.gz" \
    && ok "the same bytes on a second run" || no "the same bytes on a second run"

# The tag moved to another commit.
printf 'more\n' >> "$up/README"
git -C "$up" -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false commit -qam two
git -C "$up" tag -f v1.8.1 >/dev/null
if k2_fetch_source_archives "$WORK/d2" >/dev/null 2>"$WORK/err"; then
    no "a moved tag stops it"
else
    grep -qF "the pin in scripts/lib/release-source-archives.sh says $K2_LIBGIT2_COMMIT" "$WORK/err" \
        && ok "a moved tag stops it, naming the pinned commit" || no "a moved tag names the pinned commit"
    [ ! -e "$WORK/d2/libgit2-1.8.1-source.tar.gz" ] && ok "no archive of a moved tag" || no "no archive of a moved tag"
fi
reset_pins

# The tar's content differs from its pin.
K2_LIBGIT2_TAR_SHA256="$(printf 'other' | sha)"
if k2_fetch_source_archives "$WORK/d3" >/dev/null 2>"$WORK/err"; then
    no "a tar sha256 mismatch stops it"
else
    grep -qF "(uncompressed tar); the pin" "$WORK/err" && ok "a tar sha256 mismatch stops it" || no "a tar sha256 mismatch says so"
    [ -z "$(ls -A "$WORK/d3")" ] && ok "nothing is left after a tar mismatch" || no "nothing is left after a tar mismatch"
fi
reset_pins

# The tag can't be fetched.
K2_LIBGIT2_REPO="$WORK/no-such-repo"
if k2_fetch_source_archives "$WORK/d4" >/dev/null 2>"$WORK/err"; then
    no "an unreachable repository stops it"
else
    grep -qF "could not fetch v1.8.1" "$WORK/err" && ok "an unreachable repository stops it" || no "an unreachable repository says so"
fi
K2_LIBGIT2_REPO="$up"

# The .crate's sha256 differs, or it can't be downloaded.
K2_SOURCE_ARCHIVES=("b.crate|https://example.invalid/b|$(printf 'other bytes' | sha)|B source")
if k2_fetch_source_archives "$WORK/d5" >/dev/null 2>"$WORK/err"; then
    no "a .crate sha256 mismatch stops it"
else
    grep -qF "b.crate from https://example.invalid/b is sha256" "$WORK/err" && ok "a .crate sha256 mismatch stops it" || no "a .crate mismatch says so"
    [ ! -e "$WORK/d5/b.crate" ] && [ ! -e "$WORK/d5/b.crate.part" ] && ok "no mismatched .crate is left" || no "no mismatched .crate is left"
fi
K2_SOURCE_ARCHIVES=("b.crate|https://example.invalid/down|$(printf 'x' | sha)|B source")
if k2_fetch_source_archives "$WORK/d6" >/dev/null 2>"$WORK/err"; then
    no "a failed download stops it"
else
    grep -qF "could not download b.crate" "$WORK/err" && ok "a failed download stops it" || no "a failed download says which"
fi

echo ""
echo "release source archives: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
