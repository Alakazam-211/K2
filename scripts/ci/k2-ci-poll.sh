#!/usr/bin/env bash
# k2-ci-poll.sh — the K2 Compute CI gate's trigger (plan-ci-release-on-
# k2-compute-v1 §3; prd-ci-release-on-compute-stage1-v1 §4).
#
# On z3mbpZ it runs as a K2 scheduled job in the k2-ci workspace (a CI
# checkout, not the release one):
#   k2 job add k2-ci-poll --cron '*/2 * * * *' --timeout 10m --workspace k2-ci -- bash scripts/ci/k2-ci-poll.sh --once
# then, before its first tick, the statuses token (post-status.sh):
#   k2 vault grant K2_CI_STATUSES_TOKEN --to job:k2-ci-poll --as env --env-name CI_STATUSES_TOKEN
# (docs/ci-on-k2-compute.md). Not a K2 heartbeat (that wakes an LLM and
# costs tokens), not a git hook (misses pushes from other machines).
#
#   scripts/ci/k2-ci-poll.sh [--once] [--dry-run]
#   scripts/ci/k2-ci-poll.sh --mark-seen
#
# A pass never waits for a job, so it ends in seconds:
#   1. reap: every run in flight (run-checks.sh --reap): one whose jobs
#      all ended gets its final statuses; one past its deadline gets error.
#   2. submit: while fewer than K2_CI_MAX_INFLIGHT runs are in flight
#      (default 2), the first branch tip not seen yet (main first, then
#      integ/* by name) is marked seen and submitted
#      (run-checks.sh <sha> --submit-only --ref <branch>). A newer push to
#      a branch cancels the older push's jobs that haven't started.
# One pass at a time (a lock; a K2 job never overlaps its own runs either).
# Without --once it loops every K2_CI_POLL_SECS (default 60).
#
# --mark-seen: record every current tip as seen without checking it, so
#   turning the poller on checks only pushes from then on.
# --dry-run: statuses are printed, not posted (K2_CI_DRY_RUN=1); the jobs
#   are real. This prints one line per run ("→ pass" / "→ fail"); the
#   statuses each run would post are in $K2_CI_STATE/logs/<sha>.log.
#
# Env: K2_CI_STATE (default ~/.local/state/k2-ci): seen.tsv, inflight/,
#      runs/, logs/<sha>.log, poll.lock. K2_CI_REFS (default
#      "refs/heads/main refs/heads/integ/*"). K2_CI_MAX_INFLIGHT.
#      Statuses token: post-status.sh. Never reads ~/.k2.
set -euo pipefail

ONCE=0
MARK=0
while [ $# -gt 0 ]; do
    case "$1" in
        --once) ONCE=1; shift ;;
        --dry-run) export K2_CI_DRY_RUN=1; shift ;;
        --mark-seen) MARK=1; shift ;;
        *) echo "usage: k2-ci-poll.sh [--once] [--dry-run] | --mark-seen" >&2; exit 2 ;;
    esac
done

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STATE="${K2_CI_STATE:-$HOME/.local/state/k2-ci}"
export K2_CI_STATE="$STATE"
mkdir -p "$STATE/logs" "$STATE/runs" "$STATE/inflight"
touch "$STATE/seen.tsv"
LOCK="$STATE/poll.lock"
MAX_INFLIGHT="${K2_CI_MAX_INFLIGHT:-2}"
case "$MAX_INFLIGHT" in ''|*[!0-9]*|0) echo "K2_CI_MAX_INFLIGHT must be 1 or more" >&2; exit 2 ;; esac

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

# "<sha>\t<ref>" lines: main first, then the integ/* branches by name.
tips() {
    local refs out
    refs="${K2_CI_REFS:-refs/heads/main refs/heads/integ/*}"
    # shellcheck disable=SC2086
    out="$(set -f; git -C "$ROOT" ls-remote origin $refs)" || { echo "k2-ci-poll: ls-remote failed" >&2; return 1; }
    printf '%s\n' "$out" | awk 'NF==2 && $1 ~ /^[0-9a-f]{40}$/ { print ($2=="refs/heads/main" ? 0 : 1) "\t" $1 "\t" $2 }' \
        | sort -k1,1 -k3,3 | cut -f2-
}

seen() { grep -q "^$1	" "$STATE/seen.tsv"; }
mark() { printf '%s\t%s\t%s\n' "$1" "$2" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$STATE/seen.tsv"; }

reap_all() {
    local f sha rc
    for f in "$STATE"/inflight/*.tsv; do
        [ -e "$f" ] || continue
        sha="$(basename "$f" .tsv)"
        rc=0
        bash "$ROOT/scripts/ci/run-checks.sh" --reap "$sha" >> "$STATE/logs/$sha.log" 2>&1 || rc=$?
        case "$rc" in
            10) ;;
            0) echo "k2-ci-poll: ${sha:0:12} → pass" ;;
            *) echo "k2-ci-poll: ${sha:0:12} → fail (rc $rc; log $STATE/logs/$sha.log)" ;;
        esac
    done
}

inflight_count() { find "$STATE/inflight" -name '*.tsv' -type f | wc -l | tr -d ' '; }

submit_new() {
    local all sha ref rc
    all="$(tips)" || return 1
    while IFS=$'\t' read -r sha ref; do
        [ -n "$sha" ] || continue
        seen "$sha" && continue
        if [ "$(inflight_count)" -ge "$MAX_INFLIGHT" ]; then
            echo "k2-ci-poll: $(inflight_count) run(s) in flight (max $MAX_INFLIGHT); ${ref#refs/heads/} @ ${sha:0:12} waits"
            return 0
        fi
        mark "$sha" "$ref"
        echo "k2-ci-poll: checking $ref @ ${sha:0:12} (log $STATE/logs/$sha.log)"
        rc=0
        bash "$ROOT/scripts/ci/run-checks.sh" "$sha" --submit-only --ref "$ref" >> "$STATE/logs/$sha.log" 2>&1 || rc=$?
        [ "$rc" = 0 ] || echo "k2-ci-poll: ${sha:0:12}: submit failed (rc $rc; log $STATE/logs/$sha.log)" >&2
    done <<< "$all"
}

one_pass() {
    take_lock || { echo "k2-ci-poll: another pass holds $LOCK"; return 0; }
    local rc=0
    reap_all || rc=$?
    submit_new || rc=$?
    rm -rf "$LOCK"
    return "$rc"
}

if [ "$MARK" = 1 ]; then
    take_lock || { echo "k2-ci-poll: another pass holds $LOCK" >&2; exit 1; }
    n=0
    all="$(tips)" || { rm -rf "$LOCK"; exit 1; }
    while IFS=$'\t' read -r sha ref; do
        [ -n "$sha" ] || continue
        seen "$sha" && continue
        mark "$sha" "$ref"
        n=$((n + 1))
    done <<< "$all"
    rm -rf "$LOCK"
    echo "k2-ci-poll: marked $n tip(s) as seen; only new pushes are checked from now on."
    exit 0
fi

if [ "$ONCE" = 1 ]; then
    one_pass
    exit $?
fi
while :; do
    one_pass || true
    sleep "${K2_CI_POLL_SECS:-60}"
done
