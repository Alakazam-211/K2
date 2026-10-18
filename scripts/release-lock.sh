#!/usr/bin/env bash
# release-lock.sh — sourced by release.sh (macOS release Mac, z3mbpZ).
#
# z3mbpZ is also a K2 compute node. Its node policy has
#   foreign_locks = ["/var/tmp/k2-release.lock"]
# so while that file exists the node takes no new jobs. release.sh writes
# it first thing, waits for a node job that is already running to finish
# (never kills it), and removes it on every exit path, but only while the
# file is still its own (pid match).
#
# Functions (all print to stderr; none call sudo):
#   k2_release_lock_acquire <version>   write the lock, or FATAL if another
#                                       live release.sh holds it
#   k2_release_lock_release             remove it if it is still ours
#   k2_release_wait_node_idle           wait for `k2-node status` to show
#                                       `running: -` (timeout: warn, go on)
#
# Environment (tests point these at temp files and stubs):
#   K2_RELEASE_LOCK           default /var/tmp/k2-release.lock
#   K2_RELEASE_NODE_BIN       default "/Library/Application Support/K2/node/k2-node"
#   K2_RELEASE_NODE_HOME      default /var/k2node
#   K2_RELEASE_NODE_WAIT_MIN  default 20
#   K2_RELEASE_NODE_POLL_SECS default 5 (status every poll, a line every 30 s)

K2_RELEASE_LOCK="${K2_RELEASE_LOCK:-/var/tmp/k2-release.lock}"
K2_RELEASE_NODE_BIN="${K2_RELEASE_NODE_BIN:-/Library/Application Support/K2/node/k2-node}"
K2_RELEASE_NODE_HOME="${K2_RELEASE_NODE_HOME:-/var/k2node}"
K2_RELEASE_LOCK_OURS=""

_k2_lock_pid() {
    sed -n 's/^pid=//p' "$1" 2>/dev/null | head -1
}

k2_release_lock_acquire() {
    local version="${1:-}" holder tries
    [ -n "$version" ] || { echo "FATAL: k2_release_lock_acquire needs a version" >&2; return 1; }
    for tries in 1 2; do
        # noclobber: create only if absent, atomically.
        if ( set -C; printf 'started=%s\nversion=%s\npid=%s\n' \
                "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$version" "$$" > "$K2_RELEASE_LOCK" ) 2>/dev/null; then
            K2_RELEASE_LOCK_OURS=1
            echo "  Release lock set: ${K2_RELEASE_LOCK} (pid $$, v${version}). The compute node on this Mac takes no new jobs until it is removed." >&2
            return 0
        fi
        [ -e "$K2_RELEASE_LOCK" ] || { echo "FATAL: cannot write ${K2_RELEASE_LOCK}" >&2; return 1; }
        holder="$(_k2_lock_pid "$K2_RELEASE_LOCK")"
        if [ -n "$holder" ] && [ "$holder" != "$$" ] && kill -0 "$holder" 2>/dev/null; then
            echo "FATAL: ${K2_RELEASE_LOCK} is held by a running process (pid ${holder}):" >&2
            sed 's/^/FATAL:   /' "$K2_RELEASE_LOCK" >&2
            echo "FATAL: another release.sh is running on this Mac. If it is not, remove the file and retry." >&2
            return 1
        fi
        echo "  Removing a stale ${K2_RELEASE_LOCK} (pid ${holder:-unknown} is gone):" >&2
        sed 's/^/    /' "$K2_RELEASE_LOCK" >&2
        rm -f "$K2_RELEASE_LOCK" || { echo "FATAL: cannot remove stale ${K2_RELEASE_LOCK}" >&2; return 1; }
    done
    echo "FATAL: could not take ${K2_RELEASE_LOCK}" >&2
    return 1
}

k2_release_lock_release() {
    [ -n "$K2_RELEASE_LOCK_OURS" ] || return 0
    K2_RELEASE_LOCK_OURS=""
    [ -e "$K2_RELEASE_LOCK" ] || return 0
    if [ "$(_k2_lock_pid "$K2_RELEASE_LOCK")" = "$$" ]; then
        rm -f "$K2_RELEASE_LOCK"
        echo "  Release lock removed: ${K2_RELEASE_LOCK}" >&2
    else
        echo "  WARNING: ${K2_RELEASE_LOCK} is no longer ours (pid $(_k2_lock_pid "$K2_RELEASE_LOCK")); left in place." >&2
    fi
    return 0
}

k2_release_wait_node_idle() {
    local bin="$K2_RELEASE_NODE_BIN" home="$K2_RELEASE_NODE_HOME"
    local wait_min="${K2_RELEASE_NODE_WAIT_MIN:-20}" poll="${K2_RELEASE_NODE_POLL_SECS:-5}"
    local out running started now last_line=0
    if [ ! -e "$bin" ]; then
        return 0
    fi
    if [ ! -x "$bin" ] || [ ! -r "$home" ]; then
        echo "  Compute node: ${bin} or ${home} is not readable without sudo; lock is set, not waiting for a running job." >&2
        return 0
    fi
    started="$(date +%s)"
    while :; do
        if ! out="$("$bin" status --home "$home" 2>&1)"; then
            echo "  Compute node: \`k2-node status\` failed (${out%%$'\n'*}); lock is set, not waiting for a running job." >&2
            return 0
        fi
        running="$(printf '%s\n' "$out" | sed -n 's/^[[:space:]]*running:[[:space:]]*//p' | head -1)"
        if [ -z "$running" ]; then
            echo "  Compute node: \`k2-node status\` has no 'running:' line; lock is set, not waiting." >&2
            return 0
        fi
        if [ "$running" = "-" ]; then
            echo "  Compute node idle (running: -)." >&2
            return 0
        fi
        now="$(date +%s)"
        if [ $((now - started)) -ge $((wait_min * 60)) ]; then
            echo "  WARNING: compute node job still running after ${wait_min} min (running: ${running}). Not killing it; the release continues and the build shares this Mac with it." >&2
            echo "  WARNING: see: \"${bin}\" status --home ${home}" >&2
            return 0
        fi
        if [ "$last_line" -eq 0 ] || [ $((now - last_line)) -ge 30 ]; then
            echo "  Waiting for the compute node job to finish (running: ${running}, $(( (now - started) / 60 )) of ${wait_min} min)..." >&2
            last_line="$now"
        fi
        sleep "$poll"
    done
}
