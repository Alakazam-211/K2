#!/usr/bin/env bash
# Permission cards: rewrite the golden samples from the real daemon code
# (PRD prd-agent-settings-cards-v1 rev 3.1 §13.4, §17.10).
#
#   scripts/permission-cards/update-samples.sh            # rewrite, then re-run the drift test
#   scripts/permission-cards/update-samples.sh --print-tar
#       also print the folder as a base64 tar.gz between "=====SAMPLES-TAR-BEGIN"
#       and "=====SAMPLES-TAR-END" (for a compute node: copy the block back and
#       run scripts/permission-cards/unpack-samples.sh <log>).
#
# Review the diff afterwards: a changed sample is the phone contract changing.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
dir="crates/k2-daemon/tests/golden/permission_cards"

print_tar=0
for a in "$@"; do
  case "$a" in
    --print-tar) print_tar=1 ;;
    *) echo "usage: $0 [--print-tar]" >&2; exit 2 ;;
  esac
done

UPDATE_PERMISSION_SAMPLES=1 cargo test -p k2-daemon --lib permission_samples -- --nocapture
# The drift test must now pass on what was just written.
cargo test -p k2-daemon --lib permission_samples

echo "samples: $(find "$dir" -name '*.json' | wc -l | tr -d ' ') files in $dir"
if [ "$print_tar" = 1 ]; then
  echo "=====SAMPLES-TAR-BEGIN"
  tar czf - "$dir" | base64
  echo "=====SAMPLES-TAR-END"
fi
