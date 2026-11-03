#!/usr/bin/env bash
# arch-upload-pkg.sh — put the Arch package on its GitHub release without ever
# changing a published file's bytes. Called by .github/workflows/arch-package.yml.
#
#   scripts/arch-upload-pkg.sh <tag> <path to k2-<version>-x86_64.pkg.tar.zst>
#
# The release asset is a stable file other people pin by sha256: an Omarchy
# recipe (packaging/omarchy/k2-bin) records its checksum, and their rule is
# "never reuse a published filename for different bytes". makepkg output is not
# byte-for-byte reproducible (it records the build date), so a rebuild of the
# same tag gives different bytes. --clobber would swap them in silently.
#
#   no such asset yet      upload it
#   same bytes already up  say so, upload nothing, exit 0 (a re-run)
#   different bytes up     exit 1, and say what to do; nothing changes
#
# Environment: K2_RELEASE_REPO (default Alakazam-211/K2), GH_TOKEN for gh.
set -euo pipefail

TAG="${1:-}"
PKG="${2:-}"
REPO="${K2_RELEASE_REPO:-Alakazam-211/K2}"

[ $# -eq 2 ] && [ -n "$TAG" ] && [ -n "$PKG" ] || {
    echo "Usage: $0 <tag> <k2-<version>-x86_64.pkg.tar.zst>" >&2
    exit 2
}
[ -s "$PKG" ] || { echo "::error::$PKG is missing or empty" >&2; exit 1; }

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

NAME="$(basename "$PKG")"
HAVE="$(sha256_of "$PKG")"
# A failure here is a failure: without the list we can't tell whether an upload
# would replace anything.
ASSETS="$(gh release view "$TAG" --repo "$REPO" --json assets --jq '.assets[].name')"

if ! grep -qxF "$NAME" <<<"$ASSETS"; then
    gh release upload "$TAG" "$PKG" --repo "$REPO"
    echo "Uploaded $NAME to $TAG (sha256 $HAVE)."
    exit 0
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/k2-arch-upload.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
gh release download "$TAG" --repo "$REPO" --pattern "$NAME" --dir "$WORK"
[ -s "$WORK/$NAME" ] || { echo "::error::$TAG lists $NAME but the download gave no file" >&2; exit 1; }
PUBLISHED="$(sha256_of "$WORK/$NAME")"

if [ "$PUBLISHED" = "$HAVE" ]; then
    echo "$TAG already has these exact $NAME bytes (sha256 $HAVE); not uploading again."
    exit 0
fi

{
    echo "::error::$TAG already has a DIFFERENT $NAME; a published package never changes bytes."
    echo "  Published: $PUBLISHED"
    echo "  This build: $HAVE"
    echo "  If the published file is good and only its SHA256SUMS line is missing, add that line from the published file:"
    echo "    gh release download $TAG --repo $REPO --pattern $NAME"
    echo "    scripts/linux-sums.sh line $NAME > arch.sums && scripts/linux-sums.sh merge $TAG arch.sums"
    echo "  To ship a fix, make a new release. Omarchy's recipe may already pin the published sha256."
} >&2
exit 1
