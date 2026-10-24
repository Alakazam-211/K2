#!/usr/bin/env bash
# post-status.sh — one GitHub commit status for the K2 Compute CI gate.
#
#   scripts/ci/post-status.sh <sha> pending|success|failure|error <context> <description>
#
# Token: a fine-grained PAT with ONLY "Commit statuses: write" on
# Alakazam-211/K2, never the owner's gh login. From K2_CI_STATUS_TOKEN, or
# on macOS the login keychain item service "k2-ci-statuses", account
# "github" (add it once: security add-generic-password -s k2-ci-statuses
# -a github -w). The token goes to curl on stdin (-K -), never argv.
# No target_url: logs stay in K2 (k2 compute logs <job>).
#
# K2_CI_DRY_RUN=1 prints the status instead of posting (shadow runs,
# tests). Env: K2_CI_REPO_SLUG (default Alakazam-211/K2),
# K2_CI_API (default https://api.github.com).
set -euo pipefail

sha="${1:-}" state="${2:-}" context="${3:-}" desc="${4:-}"
usage() { echo "usage: post-status.sh <sha> pending|success|failure|error <context> <description>" >&2; exit 2; }
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || usage
case "$state" in pending|success|failure|error) ;; *) usage ;; esac
[[ "$context" =~ ^k2-compute/[a-z0-9-]+$ ]] || usage
desc="${desc:0:140}"

body="$(python3 -c 'import json,sys; print(json.dumps({"state": sys.argv[1], "context": sys.argv[2], "description": sys.argv[3]}))' "$state" "$context" "$desc")"
slug="${K2_CI_REPO_SLUG:-Alakazam-211/K2}"

if [ "${K2_CI_DRY_RUN:-0}" = "1" ]; then
    echo "status (dry run) $slug@${sha:0:12}: $body"
    exit 0
fi

token="${K2_CI_STATUS_TOKEN:-}"
if [ -z "$token" ] && command -v security >/dev/null 2>&1; then
    token="$(security find-generic-password -s k2-ci-statuses -a github -w 2>/dev/null || true)"
fi
[ -n "$token" ] || { echo "FATAL: no statuses token (K2_CI_STATUS_TOKEN or keychain item k2-ci-statuses)" >&2; exit 1; }

printf 'header = "Authorization: Bearer %s"\n' "$token" | curl -sS -f -K - \
    -X POST "${K2_CI_API:-https://api.github.com}/repos/$slug/statuses/$sha" \
    -H "Accept: application/vnd.github+json" -H "X-GitHub-Api-Version: 2022-11-28" \
    -H "Content-Type: application/json" --data "$body" -o /dev/null \
    || { echo "FATAL: GitHub refused the $context status for ${sha:0:12}" >&2; exit 1; }
echo "status $slug@${sha:0:12}: $state $context — $desc"
