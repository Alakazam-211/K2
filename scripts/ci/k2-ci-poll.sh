#!/usr/bin/env bash
# k2-ci-poll.sh — the K2 Compute CI gate's trigger (plan-ci-release-on-
# k2-compute-v1 §3). A LaunchAgent on z3mbpZ runs it every 60 s
# (scripts/ci/dev.k2.ci-poll.plist; docs/ci-on-k2-compute.md). Not a K2
# heartbeat (that wakes an LLM and costs tokens), not a git hook (misses
# pushes from other machines).
#
#   scripts/ci/k2-ci-poll.sh [--once] [--dry-run]
#
# Each pass: `git ls-remote origin` for main and integ/* tips; the first
# tip it hasn't seen (main first) is marked seen and checked with
# run-checks.sh (statuses k2-compute/*). One run at a time (a lock; launchd
# never starts a second copy of a running job either). Tips that move
# while a run is on are superseded: only the newest tip of each branch is
# checked. Without --once it loops every K2_CI_POLL_SECS (default 60).
#
# Env: K2_CI_STATE (default ~/.local/state/k2-ci): seen.tsv, runs/,
#      logs/<sha>.log, poll.lock. K2_CI_REFS (default
#      "refs/heads/main refs/heads/integ/*"). --dry-run = K2_CI_DRY_RUN=1
#      (statuses printed, not posted). Never reads ~/.k2.
set -euo pipefail

ONCE=0
while [ $# -gt 0 ]; do
    case "$1" in
        --once) ONCE=1; shift ;;
        --dry-run) export K2_CI_DRY_RUN=1; shift ;;
        *) echo "usage: k2-ci-poll.sh [--once] [--dry-run]" >&2; exit 2 ;;
    esac
done

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STATE="${K2_CI_STATE:-$HOME/.local/state/k2-ci}"
export K2_CI_STATE="$STATE"
mkdir -p "$STATE/logs" "$STATE/runs"
touch "$STATE/seen.tsv"
LOCK="$STATE/poll.lock"

take_lock() {
    if mkdir "$LOCK" 2>/dev/null; then
        echo $$ > "$LOCK/pid"
        return 0
    fi
    local pid
    pid="$(cat "$LOCK/pid" 2>/dev/null || true)"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        return 1
    fi
    # A lock left by a dead poller.
    rm -rf "$LOCK"
    mkdir "$LOCK" 2>/dev/null && echo $$ > "$LOCK/pid"
}

one_pass() {
    take_lock || { echo "k2-ci-poll: another run holds $LOCK"; return 0; }
    local rc=0
    scan || rc=$?
    rm -rf "$LOCK"
    return "$rc"
}

scan() {
    local refs tips sha ref
    # shellcheck disable=SC2086
    refs="${K2_CI_REFS:-refs/heads/main refs/heads/integ/*}"
    # shellcheck disable=SC2086
    tips="$(set -f; git -C "$ROOT" ls-remote origin $refs)" || { echo "k2-ci-poll: ls-remote failed" >&2; return 1; }
    # main first, then the integ/* branches by name.
    tips="$(printf '%s\n' "$tips" | awk 'NF==2 { print ($2=="refs/heads/main" ? 0 : 1) "\t" $1 "\t" $2 }' | sort -k1,1 -k3,3 | cut -f2-)"
    while IFS=$'\t' read -r sha ref; do
        [ -n "$sha" ] || continue
        [[ "$sha" =~ ^[0-9a-f]{40}$ ]] || continue
        if grep -q "^$sha	" "$STATE/seen.tsv"; then
            continue
        fi
        printf '%s\t%s\t%s\n' "$sha" "$ref" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$STATE/seen.tsv"
        echo "k2-ci-poll: checking $ref @ ${sha:0:12} (log $STATE/logs/$sha.log)"
        local rc=0
        bash "$ROOT/scripts/ci/run-checks.sh" "$sha" > "$STATE/logs/$sha.log" 2>&1 || rc=$?
        echo "k2-ci-poll: $ref @ ${sha:0:12} → $([ "$rc" = 0 ] && echo pass || echo "fail (rc $rc)")"
        return 0 # one run per pass; the next pass picks up the next tip
    done <<< "$tips"
    return 0
}

if [ "$ONCE" = 1 ]; then
    one_pass
    exit $?
fi
while :; do
    one_pass || true
    sleep "${K2_CI_POLL_SECS:-60}"
done
