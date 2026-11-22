#!/usr/bin/env bash
# Your signing step (SECRETS). pipeline/driver.sh ship calls it after
# checking that <out> is a real build of your brand, unchanged:
#
#   bash hooks/sign.sh <out> <distributor.conf>
#
# It signs with YOUR identity, never K5's. The reference signs each binary
# in <out>/bin, and SHA256SUMS, with your update key (minisign), in the
# form K5's updaters verify: a prehashed signature whose trusted comment
# is `timestamp:<unix>`, `file:<asset>` and `version:<x.y.z>`, tab
# separated. Each signature is checked against update_pubkey before the
# step ends.
#
# Secrets (from your CI's secret store, never from a file in the repo):
#   UPDATE_MINISIGN_KEY       your minisign secret key file's text
#   UPDATE_MINISIGN_PASSWORD  its password
#
# When your build also makes desktop apps, add their signing here, each
# with its own secrets, for example:
#   macOS    your Developer ID Application certificate and team id, then
#            notarizing with your App Store Connect key
#   Windows  your code signing certificate (Authenticode)
set -euo pipefail

[ $# = 2 ] || { echo "usage: sign.sh <out> <distributor.conf>" >&2; exit 2; }
OUT="$1"
CONF="$2"
die() { echo "sign: $*" >&2; exit 1; }

: "${UPDATE_MINISIGN_KEY:?sign: UPDATE_MINISIGN_KEY is not set (a secret)}"
: "${UPDATE_MINISIGN_PASSWORD:?sign: UPDATE_MINISIGN_PASSWORD is not set (a secret)}"
command -v minisign >/dev/null || die "needs minisign on PATH"

pubkey="$(sed -n 's/^update_pubkey[[:space:]]*=[[:space:]]*//p' "$CONF")"
version="$(sed -n 's/^version = //p' "$OUT/manifest.txt")"
[ -n "$pubkey" ] || die "no update_pubkey in $CONF"
[ -n "$version" ] || die "no version in $OUT/manifest.txt"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
umask 077
printf '%s\n' "$UPDATE_MINISIGN_KEY" > "$work/update.key"
printf 'untrusted comment: your update key\n%s\n' "$pubkey" > "$work/update.pub"

sign_one() { # sign_one <file>
    local f="$1" name comment got
    name="$(basename "$f")"
    comment="$(printf 'timestamp:%s\tfile:%s\tversion:%s' "$(date +%s)" "$name" "$version")"
    printf '%s\n' "$UPDATE_MINISIGN_PASSWORD" |
        minisign -S -s "$work/update.key" -m "$f" -x "$f.minisig" -t "$comment" >/dev/null
    got="$(minisign -V -m "$f" -p "$work/update.pub" -x "$f.minisig" -Q)" \
        || die "$name: the new signature does not verify with update_pubkey"
    [ "$(printf '%s\n' "$got" | tail -n 1)" = "$comment" ] || die "$name: the trusted comment did not round-trip"
    echo "sign: $name"
}

for f in "$OUT"/bin/*; do
    case "$f" in *.minisig) continue ;; esac
    sign_one "$f"
done
sign_one "$OUT/SHA256SUMS"
echo "sign: done, version $version"
