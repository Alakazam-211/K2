#!/usr/bin/env bash
# The brand gate: no NEW hard-coded product name or k2.dev in Rust or
# renderer code outside the brand and hosts modules.
#
#   bash scripts/brand-gate.sh
#
# Two checks, both ratchets (a file's count may only fall):
#   1. k2.dev: scripts/garden/zone-gate.py for Rust and for the renderer,
#      ceilings in scripts/garden/zone-ceilings*.txt. Use k2_core::hosts or
#      @/lib/hosts.
#   2. the product's names (K2, K5, the release repo): scripts/garden/brand-gate.py,
#      allowlist scripts/brand-allowlist.txt, one reason per row (keeper or
#      debt). Use k2_core::brand or @/lib/brand.
# Run in CI by .github/workflows/checks.yml and scripts/ci/checks.sh.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
python3 "$HERE/scripts/garden/zone-gate.py"
python3 "$HERE/scripts/garden/zone-gate.py" --area renderer
python3 "$HERE/scripts/garden/brand-gate.py"
