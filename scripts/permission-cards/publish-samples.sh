#!/usr/bin/env bash
# Permission cards: copy the repo's golden samples to the shared folder the
# Companion (phone) team reads (PRD rev 3.1 §13.4, §17.10). Run by
# integration (I) after the drift test passes on the integration branch.
#
#   scripts/permission-cards/publish-samples.sh [dest]
#   dest defaults to /Users/Shared/k2-companion/permission-cards/samples
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
src="$root/crates/k2-daemon/tests/golden/permission_cards"
dest="${1:-/Users/Shared/k2-companion/permission-cards/samples}"

if [ ! -f "$src/index.json" ]; then
  echo "no samples in $src (run scripts/permission-cards/update-samples.sh)" >&2
  exit 1
fi
mkdir -p "$dest"
# Exactly the repo's files: drop anything the repo no longer writes.
find "$dest" -maxdepth 1 -name '*.json' -type f -delete
cp "$src"/*.json "$dest"/
[ -f "$src/README.md" ] && cp "$src/README.md" "$dest/README.md"
"$root/scripts/permission-cards/check-shared-samples.sh" "$dest"
echo "published $(find "$dest" -maxdepth 1 -name '*.json' | wc -l | tr -d ' ') files to $dest"
