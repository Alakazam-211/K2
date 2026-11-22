#!/usr/bin/env bash
# Your publishing step (SECRETS). pipeline/driver.sh ship calls it after
# signing, only when publish = on:
#
#   bash hooks/publish.sh <out> <distributor.conf>
#
# It puts the signed build on YOUR update feed (feed_url), never K5's.
# Where the feed lives is yours to choose (your own web server, a storage
# bucket, releases in your own repo); this reference uploads each file
# with HTTP PUT to FEED_UPLOAD_URL/<version>/<file>, then the manifest
# last, so a client never sees a version before its files are there.
# Replace the upload with your host's own tool if it has one.
#
# Secrets (from your CI's secret store):
#   FEED_UPLOAD_URL    where uploads go (may differ from feed_url, which
#                      is what your users' builds read)
#   FEED_UPLOAD_TOKEN  a token that may write there, and nothing else
set -euo pipefail

[ $# = 2 ] || { echo "usage: publish.sh <out> <distributor.conf>" >&2; exit 2; }
OUT="$1"
die() { echo "publish: $*" >&2; exit 1; }

: "${FEED_UPLOAD_URL:?publish: FEED_UPLOAD_URL is not set (a secret)}"
: "${FEED_UPLOAD_TOKEN:?publish: FEED_UPLOAD_TOKEN is not set (a secret)}"
version="$(sed -n 's/^version = //p' "$OUT/manifest.txt")"
[ -n "$version" ] || die "no version in $OUT/manifest.txt"

put() { # put <file> <name>
    curl --fail --silent --show-error --retry 3 \
        -H "Authorization: Bearer $FEED_UPLOAD_TOKEN" \
        --upload-file "$1" "${FEED_UPLOAD_URL%/}/$version/$2" >/dev/null \
        || die "upload of $2 failed"
    echo "publish: $version/$2"
}

for f in "$OUT"/bin/* "$OUT/SHA256SUMS" "$OUT/SHA256SUMS.minisig"; do
    put "$f" "$(basename "$f")"
done
put "$OUT/manifest.txt" manifest.txt
echo "publish: done, version $version"
