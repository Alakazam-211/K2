#!/bin/sh
# Vendor the patched engine (k2-yrs: Yrs v0.28.0 plus the k2 patches) into vendor/yrs, or
# check that vendor/yrs is exactly a tag's copy. See docs/YRS-FORK.md.
#
#   tools/vendor-yrs.sh <k2-yrs clone> <tag>           write vendor/yrs from the tag
#   tools/vendor-yrs.sh --check <k2-yrs clone> [<tag>]  rebuild it in a scratch dir and compare
#                                                     (the tag defaults to the recorded one)
#
# What is copied: the tag's yrs/ (the crate) as vendor/yrs/, and the repo's LICENSE (Yrs's MIT
# licence) and NOTICE (what the fork changed) beside it. Left out: the engine's own tests,
# yrs/tests/ (they run in k2-yrs; the kit never builds a dependency's tests, and compute jobs
# never carried them), whatever a tag holds there, so older tags still re-vendor.
# vendor/yrs/VENDORED records the tag, its commit, the tree hash of its yrs/, what was left
# out, a sha256 of every vendored file, and how a compute job rebuilds src/ without carrying
# it: the upstream crate on crates.io (yrs 0.28.0, its sha256) plus vendor/yrs/FORK.patch (the
# fork's diff of yrs/src from the upstream tag), checked against `src-sha256`. --check recomputes all of it. Files under
# vendor/yrs are never edited by hand.
set -eu
CHECK=0
if [ "${1:-}" = --check ]; then CHECK=1; shift; fi
SRC=${1:?usage: tools/vendor-yrs.sh [--check] <k2-yrs clone> <tag>}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DEST=$ROOT/vendor/yrs
# left out when present: the engine's tests (they run in k2-yrs)
EXCLUDE="yrs/tests/"
# the upstream release the fork starts from: its tag in k2-yrs, and its crate on crates.io
UPSTREAM_TAG=v0.28.0
CRATE=yrs-0.28.0
CRATE_SHA256=52c70dc8beca8666c77612a96889106ca3cd65318609721f464624ff79685da9
if [ -n "${2:-}" ]; then TAG=$2
elif [ "$CHECK" = 1 ] && [ -f "$DEST/VENDORED" ]; then TAG=$(sed -n 's/^tag //p' "$DEST/VENDORED")
else echo "a tag is needed" >&2; exit 2; fi
[ "$(git -C "$SRC" cat-file -t "$TAG" 2>/dev/null)" = tag ] || { echo "$TAG is not an annotated tag in $SRC" >&2; exit 2; }
COMMIT=$(git -C "$SRC" rev-parse "$TAG^{commit}")
TREE=$(git -C "$SRC" rev-parse "$TAG:yrs")
if command -v sha256sum >/dev/null; then SHA="sha256sum"; else SHA="shasum -a 256"; fi
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/x" "$TMP/out"
git -C "$SRC" archive --format=tar "$TAG" yrs LICENSE NOTICE | tar -xf - -C "$TMP/x"
rm -rf "$TMP/x/$EXCLUDE"
( cd "$TMP/x/yrs" && tar -cf - . ) | tar -xf - -C "$TMP/out"
cp "$TMP/x/LICENSE" "$TMP/x/NOTICE" "$TMP/out/"
# the fork's change to src/ from the upstream release (a compute job's bundle carries this, not
# src/: tools/test-bundle.sh)
git -C "$SRC" diff --no-color --no-ext-diff --src-prefix=a/ --dst-prefix=b/ "$UPSTREAM_TAG" "$TAG" -- yrs/src > "$TMP/out/FORK.patch"
SRCSUM=$(cd "$TMP/out/src" && find . -type f | LC_ALL=C sort | sed 's|^\./||' | while read -r f; do $SHA "$f"; done | $SHA | cut -d' ' -f1)
FILES=$(cd "$TMP/out" && find . -type f | LC_ALL=C sort | sed 's|^\./||')
SUM=$(cd "$TMP/out" && for f in $FILES; do $SHA "$f"; done | $SHA | cut -d' ' -f1)
{
  echo "# vendor/yrs: written by tools/vendor-yrs.sh; do not edit (docs/YRS-FORK.md)"
  echo "upstream Yrs v0.28.0 (https://github.com/y-crdt/y-crdt), MIT, plus the k2 patches"
  echo "tag $TAG"
  echo "commit $COMMIT"
  echo "yrs-tree $TREE"
  echo "excluded $EXCLUDE"
  echo "upstream-tag $UPSTREAM_TAG $(git -C "$SRC" rev-parse "$UPSTREAM_TAG^{commit}")"
  echo "crate $CRATE $CRATE_SHA256"
  echo "src-sha256 $SRCSUM"
  echo "files $(echo "$FILES" | wc -l | tr -d ' ')"
  echo "files-sha256 $SUM"
} > "$TMP/out/VENDORED"
if [ "$CHECK" = 1 ]; then
  if diff -r "$TMP/out" "$DEST" >/dev/null; then
    echo "vendor/yrs is $TAG ($COMMIT), files-sha256 $SUM"
  else
    diff -r "$TMP/out" "$DEST" | head -20 >&2
    echo "vendor/yrs differs from $TAG ($COMMIT)" >&2; exit 1
  fi
else
  rm -rf "$DEST"
  mkdir -p "$DEST"
  ( cd "$TMP/out" && tar -cf - . ) | tar -xf - -C "$DEST"
  echo "vendor/yrs <- $TAG ($COMMIT, yrs tree $TREE): $(echo "$FILES" | wc -l | tr -d ' ') files, files-sha256 $SUM"
fi
