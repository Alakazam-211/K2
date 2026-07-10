#!/usr/bin/env bash
# private-docs-gate.sh — the K2 repo is PUBLIC; the cloud/monetization
# layer is documented ONLY in the private cloud repo (policy 2026-07-10:
# public repo = how K2 works; relay/fleet/supply/pricing/billing = private).
# `.k2/` is gitignored, so these docs can only leak via a deliberate
# `git add -f` — this gate makes that a CI failure instead of an incident.
#
# Wired into .github/workflows/checks.yml next to k2so-gate.sh.
set -euo pipefail

# Deny-list of TRACKED doc names that mark cloud-business content.
PATTERN='(cloud|billing|fleet|relay|bare-metal|master-plan|server-migration|server-upgrade|hosted-servers|federations)'

hits=$(git ls-files '.k2/prds/' '.k2/notes/' | grep -iE "$PATTERN" || true)
if [ -n "$hits" ]; then
    echo "private-docs-gate: FAIL — cloud-business docs are tracked in the PUBLIC repo:" >&2
    echo "$hits" | sed 's/^/  /' >&2
    echo "Move them to the private cloud repo (docs/prds/) and 'git rm --cached' here." >&2
    exit 1
fi
echo "private-docs-gate: OK"
