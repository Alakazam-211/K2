#!/usr/bin/env bash
# Quiet progress for release wait loops (0.46.4, Rosson 2026-10-23): a loop
# that polls every N seconds prints its "waiting for …" line ONCE, then one
# dot per poll, and a short status line every K2_WAIT_STATUS_SECS (default
# 300 = 5 min) — not one line per poll.
#
#   k2_wait_tick "<message>"   once per poll that is still waiting
#   k2_wait_done               when the wait ends (success or giving up)
#
# After a tick, K2_WAIT_SAID is 1 when that tick printed a whole line (the
# first one or a status line), so a caller can add detail lines under it;
# 0 when it printed only a dot. Everything goes to stderr. bash 3.2-safe
# (macOS /bin/bash). Sourced by release.sh, linux-sums.sh and the sign-*
# scripts.

K2_WAIT_SAID=0
_K2_WAIT_T0=""
_K2_WAIT_LAST=""
_K2_WAIT_DOTS=0

# Seconds since the epoch (a function so tests can drive a fake clock).
k2_wait_now() { date +%s; }

# 45 → "45s", 75 → "1m15s", 3900 → "1h05m".
k2_wait_elapsed() {
    local s="$1"
    if [ "$s" -ge 3600 ]; then
        printf '%dh%02dm' $((s / 3600)) $(((s % 3600) / 60))
    elif [ "$s" -ge 60 ]; then
        printf '%dm%02ds' $((s / 60)) $((s % 60))
    else
        printf '%ds' "$s"
    fi
}

k2_wait_tick() {
    local msg="$1" now
    now="$(k2_wait_now)"
    if [ -z "$_K2_WAIT_T0" ]; then
        _K2_WAIT_T0="$now"
        _K2_WAIT_LAST="$now"
        _K2_WAIT_DOTS=0
        K2_WAIT_SAID=1
        printf '%s\n' "$msg" >&2
        return 0
    fi
    if [ $((now - _K2_WAIT_LAST)) -ge "${K2_WAIT_STATUS_SECS:-300}" ]; then
        _K2_WAIT_LAST="$now"
        if [ "$_K2_WAIT_DOTS" = 1 ]; then printf '\n' >&2; fi
        _K2_WAIT_DOTS=0
        K2_WAIT_SAID=1
        printf '  still waiting (%s): %s\n' "$(k2_wait_elapsed $((now - _K2_WAIT_T0)))" "$msg" >&2
        return 0
    fi
    K2_WAIT_SAID=0
    _K2_WAIT_DOTS=1
    printf '.' >&2
}

k2_wait_done() {
    if [ "$_K2_WAIT_DOTS" = 1 ]; then printf '\n' >&2; fi
    _K2_WAIT_T0=""
    _K2_WAIT_LAST=""
    _K2_WAIT_DOTS=0
    K2_WAIT_SAID=0
}
