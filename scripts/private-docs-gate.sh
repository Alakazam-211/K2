#!/usr/bin/env bash
# private-docs-gate.sh — the K2 repo is PUBLIC; the cloud/monetization
# layer is documented ONLY in the private cloud repo (policy 2026-07-10,
# sharpened 2026-07-11: public repo = the K2 software/daemon; anything in
# OUR control layer — relay/edge, fleet, supply, pricing/billing, and the
# DNS/nameserver layer — is private).
# `.k2/` is gitignored, so these docs can only leak via a deliberate
# `git add -f` — this gate makes that a CI failure instead of an incident.
#
# Wired into .github/workflows/checks.yml next to k2-home-gate.sh, and the
# K2 Compute gate (scripts/ci/checks.sh web).
#
# It reads the checkout's files, not `git ls-files`: a K2 Compute node's
# checkout belongs to the node user, not the job user, so git there refuses
# the repo ("dubious ownership"). The gate runs on a clean checkout of one
# commit (a GitHub runner, a node's worktree at the sha), where every file
# under the gitignored `.k2/` is a tracked one. On a dev checkout your own
# local, untracked notes under `.k2/prds/` count too: run it in CI.
set -euo pipefail
cd "$(dirname "$0")/.."

# Deny-list of TRACKED doc names that mark cloud-business content.
PATTERN='(cloud|billing|fleet|relay|bare-metal|master-plan|server-migration|server-upgrade|hosted-servers|federations|nameserver|dns-edge|edge-r2|resilient-edge)'

files=""
for d in .k2/prds .k2/notes; do
    if [ -d "$d" ]; then
        files+="$(find "$d" \( -type f -o -type l \) -print)"$'\n'
    fi
done
hits=$(printf '%s' "$files" | grep -iE "$PATTERN" || true)
if [ -n "$hits" ]; then
    echo "private-docs-gate: FAIL — cloud-business docs are tracked in the PUBLIC repo:" >&2
    echo "$hits" | sed 's/^/  /' >&2
    echo "Move them to the private cloud repo (docs/prds/) and 'git rm --cached' here." >&2
    exit 1
fi
echo "private-docs-gate: OK"
