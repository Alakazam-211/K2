#!/usr/bin/env bash
# Permission cards: unpack the samples a compute node printed with
# `update-samples.sh --print-tar` into this checkout.
#
#   k2 compute logs <job> --raw > /tmp/job.log
#   scripts/permission-cards/unpack-samples.sh /tmp/job.log
set -euo pipefail

log="${1:?usage: $0 <job log with the =====SAMPLES-TAR block>}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
dir="crates/k2-daemon/tests/golden/permission_cards"

block="$(awk '/^=====SAMPLES-TAR-BEGIN/{on=1; next} /^=====SAMPLES-TAR-END/{on=0} on' "$log")"
if [ -z "$block" ]; then
  echo "no =====SAMPLES-TAR block in $log" >&2
  exit 1
fi
rm -rf "${root:?}/$dir"
printf '%s\n' "$block" | base64 --decode | tar xzf - -C "$root"
echo "unpacked $(find "$root/$dir" -name '*.json' | wc -l | tr -d ' ') files into $dir"
