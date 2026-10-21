#!/usr/bin/env bash
# Permission cards lane F: everything lane F owns, in one run.
#   - the golden samples' drift test (rewrites them first with --update);
#   - the guest arm: App guests never get a permission card on any door;
#   - the contract rows (catalog lint, freshness, ROUTES bindings, pins).
#
#   scripts/permission-cards/verify.sh            # check only
#   scripts/permission-cards/verify.sh --update   # rewrite the samples, print them as a tar block, then check
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

if [ "${1:-}" = "--update" ]; then
  "$root/scripts/permission-cards/update-samples.sh" --print-tar
else
  cargo test -p k2-daemon --lib permission_samples
fi
cargo test -p k2-daemon --test permission_guest_hidden_integration --test contract_routes
cargo test -p k2-core --lib contract::
