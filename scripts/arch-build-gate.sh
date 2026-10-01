#!/bin/bash
# The ssh command strings expand LOCK, GATE_ID and COMMIT here on purpose.
# shellcheck disable=SC2029
#
# arch-build-gate.sh — build the Arch package on k2-arch BEFORE the tag.
# Called by scripts/release.sh (started at Step 1.6 in the background,
# waited on at Step 7.9). Safe to run standalone.
#
# Why: arch-package.yml only ran after the tag was live. 0.41.6 died there
# on the CMake 4 policy floor and 0.43.0 on a stale k2.install sha256.
# Ubuntu has linux-build-gate.sh (Step 0.6); this is the Arch twin.
#
# What it does, in order:
#   1. Writes $LOCK on k2-arch over ssh (Sew's rule: no Sew build starts
#      while that file exists or a Runner.Worker is running). If a lock
#      is already there (operator wrote it by hand), it is left alone and
#      not removed at the end.
#   2. Pushes <commit> to a throwaway branch arch-gate/<gate-id>. No
#      workflow triggers on that branch. The runner can only check out a
#      commit GitHub has.
#   3. Dispatches arch-package.yml on that branch with ref=<commit>,
#      upload=false, gate_id=<gate-id>. upload=false keeps the package as
#      a one-day workflow artifact and never touches a release.
#   4. Finds the run by its run-name ("arch-package gate <gate-id>") and
#      polls `gh run view` every $POLL seconds until it completes or the
#      timeout passes (default 45 min, counted from dispatch, so queue
#      time behind the k2-arch-ship concurrency group counts).
#   5. Always, on success, failure, timeout or SIGTERM: cancels the run if
#      it is still going, deletes the throwaway branch, removes the lock
#      if this script wrote it.
#
# Operator, before the release (wiki "CI - Linux and Arch Ship"):
#   - send Sew `HOLD k2-arch`. This script writes the lock; it does not
#     message Sew.
#   - after the post-tag arch-package run succeeds, send Sew `k2-arch
#     clear`. The tag run is a second build; write the lock again by hand
#     for it if Sew should stay off the box until then.
#
# Usage: scripts/arch-build-gate.sh <commit> <label>
#   <commit>  commit to build (release.sh passes the release preview)
#   <label>   short tag for names, e.g. v0.43.1
#
# Env:
#   K2_SKIP_ARCH_GATE=1        skip, with a loud warning
#   K2_ARCH_GATE_HOST          ssh target for the lock (k2-ci@40.160.54.134)
#   K2_ARCH_GATE_LOCK          lock path on that host (/var/tmp/k2-release.lock)
#   K2_ARCH_GATE_REPO          GitHub repo (K2_RELEASE_REPO, else Alakazam-211/K2)
#   K2_ARCH_GATE_REMOTE        git remote to push the branch to (origin)
#   K2_ARCH_GATE_TIMEOUT_MIN   minutes to wait after dispatch (45)
#   K2_ARCH_GATE_TIMEOUT_SEC   same, in seconds; wins over _MIN (tests)
#   K2_ARCH_GATE_POLL_SEC      seconds between polls (30)
#   K2_ARCH_GATE_FIND_SEC      seconds to wait for the run to appear (180)
set -euo pipefail

if [ "${K2_SKIP_ARCH_GATE:-0}" = "1" ]; then
    echo "⚠⚠ ARCH BUILD GATE SKIPPED (K2_SKIP_ARCH_GATE=1) ⚠⚠"
    echo "  An Arch packaging break will not surface until arch-package.yml"
    echo "  runs on the pushed tag, after the release is live."
    exit 0
fi

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <commit> <label>" >&2
    exit 2
fi
COMMIT_IN="$1"
LABEL="$2"

GATE_HOST="${K2_ARCH_GATE_HOST:-k2-ci@40.160.54.134}"
LOCK="${K2_ARCH_GATE_LOCK:-/var/tmp/k2-release.lock}"
REPO="${K2_ARCH_GATE_REPO:-${K2_RELEASE_REPO:-Alakazam-211/K2}}"
REMOTE="${K2_ARCH_GATE_REMOTE:-origin}"
TIMEOUT_SEC="${K2_ARCH_GATE_TIMEOUT_SEC:-$(( ${K2_ARCH_GATE_TIMEOUT_MIN:-45} * 60 ))}"
POLL_SEC="${K2_ARCH_GATE_POLL_SEC:-30}"
FIND_SEC="${K2_ARCH_GATE_FIND_SEC:-180}"
WORKFLOW="arch-package.yml"
SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=20)

COMMIT="$(git rev-parse --verify "${COMMIT_IN}^{commit}")"
GATE_ID="${LABEL}-$(date -u +%Y%m%dT%H%M%SZ)-$$"
BRANCH="arch-gate/${GATE_ID}"
RUN_TITLE="arch-package gate ${GATE_ID}"

LOCK_OWNED=0
BRANCH_PUSHED=0
RUN_ID=""
RUN_DONE=0

# Interruptible sleep: a TERM from release.sh runs the trap right away
# instead of after the sleep.
nap() {
    sleep "$1" &
    wait "$!"
}

cleanup() {
    local rc=$?
    set +e
    if [ -n "$RUN_ID" ] && [ "$RUN_DONE" = 0 ]; then
        echo "  Cancelling arch-package run ${RUN_ID} (gate did not finish)..."
        if ! gh run cancel "$RUN_ID" --repo "$REPO"; then
            echo "  WARNING: could not cancel run ${RUN_ID}. Cancel it by hand:" >&2
            echo "  WARNING:   gh run cancel ${RUN_ID} --repo ${REPO}" >&2
        fi
    fi
    if [ "$BRANCH_PUSHED" = 1 ]; then
        if git push --quiet "$REMOTE" --delete "$BRANCH"; then
            echo "  Deleted throwaway branch ${BRANCH}."
        else
            echo "  WARNING: could not delete ${BRANCH}. Delete it by hand:" >&2
            echo "  WARNING:   git push ${REMOTE} --delete ${BRANCH}" >&2
        fi
    fi
    if [ "$LOCK_OWNED" = 1 ]; then
        # Remove only our lock: the file must still name this gate id.
        if ssh "${SSH_OPTS[@]}" "$GATE_HOST" \
            "if grep -qF '${GATE_ID}' '${LOCK}'; then rm -f '${LOCK}'; fi; test ! -e '${LOCK}' || ! grep -qF '${GATE_ID}' '${LOCK}'"; then
            echo "  Removed ${LOCK} on ${GATE_HOST}."
        else
            echo "  ⚠ WARNING: could not remove ${LOCK} on ${GATE_HOST}. Sew stays blocked until it is gone:" >&2
            echo "  ⚠ WARNING:   ssh ${GATE_HOST} rm -f ${LOCK}" >&2
        fi
    fi
    exit "$rc"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT

echo "Arch build gate on k2-arch: ${COMMIT:0:12} (${LABEL}), gate id ${GATE_ID}"

# 1. Lock. set -C makes the redirect fail if the file appeared meanwhile.
lock_state="$(ssh "${SSH_OPTS[@]}" "$GATE_HOST" \
    "if [ -e '${LOCK}' ]; then echo EXISTS; else umask 022; (set -C; printf '%s\n' 'k2 release.sh arch gate ${GATE_ID} commit ${COMMIT} from $(hostname -s) at $(date -u +%Y-%m-%dT%H:%M:%SZ)' > '${LOCK}') && echo WROTE; fi")" || {
    echo "FATAL: could not write ${LOCK} on ${GATE_HOST} (ssh failed)." >&2
    echo "FATAL: the Arch gate must hold the k2-arch lock. Fix ssh, or K2_SKIP_ARCH_GATE=1." >&2
    exit 1
}
case "$lock_state" in
    WROTE)
        LOCK_OWNED=1
        echo "  Wrote ${LOCK} on ${GATE_HOST}."
        ;;
    EXISTS)
        echo "  ${LOCK} already exists on ${GATE_HOST}; leaving it (not ours, not removed after)."
        ;;
    *)
        echo "FATAL: unexpected answer writing ${LOCK} on ${GATE_HOST}: '${lock_state}'" >&2
        exit 1
        ;;
esac

# 2. Throwaway branch.
git push --quiet "$REMOTE" "${COMMIT}:refs/heads/${BRANCH}"
BRANCH_PUSHED=1
echo "  Pushed ${COMMIT:0:12} to ${REMOTE} ${BRANCH}."

# 3. Dispatch.
gh workflow run "$WORKFLOW" --repo "$REPO" --ref "$BRANCH" \
    -f ref="$COMMIT" -f upload=false -f gate_id="$GATE_ID"
started=$SECONDS
echo "  Dispatched ${WORKFLOW} (upload=false). Waiting up to $((TIMEOUT_SEC / 60)) min."

# 4a. Find the run.
while [ -z "$RUN_ID" ]; do
    if [ $((SECONDS - started)) -ge "$FIND_SEC" ]; then
        echo "FATAL: no '${RUN_TITLE}' run showed up within ${FIND_SEC}s." >&2
        echo "FATAL: check https://github.com/${REPO}/actions/workflows/${WORKFLOW}" >&2
        exit 1
    fi
    # A gh error here is retried until FIND_SEC, which then fails the gate.
    if found="$(gh run list --repo "$REPO" --workflow "$WORKFLOW" --branch "$BRANCH" \
        --event workflow_dispatch --limit 20 --json databaseId,displayTitle \
        --jq ".[] | select(.displayTitle == \"${RUN_TITLE}\") | .databaseId")"; then
        RUN_ID="${found%%$'\n'*}"
    else
        echo "  WARNING: gh run list failed; retrying." >&2
    fi
    [ -n "$RUN_ID" ] || nap 5
done
RUN_URL="https://github.com/${REPO}/actions/runs/${RUN_ID}"
echo "  Run ${RUN_ID}: ${RUN_URL}"

# 4b. Poll. A gh error is retried, but five in a row fail the gate.
gh_errors=0
last=""
while :; do
    if state="$(gh run view "$RUN_ID" --repo "$REPO" --json status,conclusion \
        --jq '.status + " " + .conclusion')"; then
        gh_errors=0
    else
        gh_errors=$((gh_errors + 1))
        echo "  WARNING: gh run view failed (${gh_errors}/5)." >&2
        if [ "$gh_errors" -ge 5 ]; then
            echo "FATAL: gh run view failed 5 times in a row for ${RUN_URL}" >&2
            exit 1
        fi
        state=""
    fi
    status="${state%% *}"
    conclusion="${state#* }"
    if [ "$status" = "completed" ]; then
        RUN_DONE=1
        break
    fi
    if [ -n "$status" ] && [ "$status" != "$last" ]; then
        echo "  $(date -u +%H:%M:%SZ) run ${RUN_ID}: ${status}"
        last="$status"
    fi
    if [ $((SECONDS - started)) -ge "$TIMEOUT_SEC" ]; then
        echo "FATAL: Arch build gate TIMED OUT after $((TIMEOUT_SEC / 60)) min (run ${RUN_ID} still ${status:-unknown})." >&2
        echo "FATAL: ${RUN_URL}" >&2
        exit 1
    fi
    nap "$POLL_SEC"
done

if [ "$conclusion" != "success" ]; then
    echo "FATAL: Arch build gate FAILED: run ${RUN_ID} concluded '${conclusion}'." >&2
    echo "FATAL: ${RUN_URL}" >&2
    echo "FATAL: gh run view ${RUN_ID} --repo ${REPO} --log-failed" >&2
    exit 1
fi
echo "  Arch build gate passed in $(((SECONDS - started) / 60)) min (run ${RUN_ID})."
