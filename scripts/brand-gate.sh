#!/usr/bin/env bash
# The brand gate: no NEW hard-coded product name, command name or k2.dev in
# Rust or renderer code outside the brand and hosts modules.
#
#   bash scripts/brand-gate.sh
#
# Three scripts, all ratchets (a count may only fall):
#   1. k2.dev: scripts/garden/zone-gate.py for Rust and for the renderer,
#      ceilings in scripts/garden/zone-ceilings*.txt. Use k2_core::hosts or
#      @/lib/hosts.
#   2. scripts/garden/brand-gate.py, one read of each file for every kind:
#      - the product's names (K2, K5, the release repo), allowlist
#        scripts/brand-allowlist.txt. Use k2_core::brand or @/lib/brand.
#      - the command's name (k2, k5): typed out as a command (cli), a
#        string that is only the name in Rust, the renderer or the bash
#        CLI's parts, or the name as a word in bash code (value: each row
#        says its class, kept or the command), a brand name built into a
#        token (glued), allowlist scripts/cli-name-allowlist.txt. Use
#        CLI_NAME or cli_name!().
#      - the bash CLI's unsure uses of the command name, one ceiling per
#        reason, scripts/garden/cli-name-unsure-ceilings.txt.
#      Every allowlist row has a reason (keeper or debt).
#   3. scripts/garden/k2-literal-gate.py: server text for a person (k2-daemon,
#      k2-core, k2-client) that types a literal `k2 <word>`, one ceiling per
#      family, scripts/garden/k2-literal-ceilings.txt. Rebrand step B7 turns
#      these into {cli} templates; this keeps new ones out meanwhile.
# Run in CI by .github/workflows/checks.yml and scripts/ci/checks.sh.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
python3 "$HERE/scripts/garden/zone-gate.py"
python3 "$HERE/scripts/garden/zone-gate.py" --area renderer
python3 "$HERE/scripts/garden/brand-gate.py"
python3 "$HERE/scripts/garden/k2-literal-gate.py"
