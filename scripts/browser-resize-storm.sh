#!/usr/bin/env bash
# Browser underlay resize-storm memory check, for a real
# screen (the build Mac), with the built K2 app running and ONE browser tab open on
# a page in the front K2 window. Not for compute nodes: they have no screen.
#
#   scripts/browser-resize-storm.sh [seconds=60] [settle=30]
#
# For `seconds` it resizes K2's front window continuously (grow and shrink,
# through System Events: the terminal needs Accessibility permission), and
# every second samples the physical footprint of the K2 app and of each
# WebKit process that started after it. Then it stops and samples for
# `settle` seconds more.
#
# Pass: total peak <= 500 MB; no WebContent process above 300 MB; total back
# within 20% of the pre-storm level within `settle` seconds of stopping.
# Prints one PASS/FAIL line per rule and exits non-zero on any FAIL. The
# samples go to ./resize-storm-<time>.csv.
set -euo pipefail
SECS="${1:-60}"
SETTLE="${2:-30}"
[ "$(uname -s)" = Darwin ] || { echo "resize-storm: macOS only" >&2; exit 2; }
[ -z "${K2_JOB_CPUS:-}" ] || { echo "resize-storm: needs a real screen, not a compute job" >&2; exit 2; }
APP_PID="$(pgrep -x k2 | head -1 || true)"
[ -n "$APP_PID" ] || APP_PID="$(pgrep -x K2 | head -1 || true)"
[ -n "$APP_PID" ] || { echo "resize-storm: K2 is not running" >&2; exit 2; }
PROC_NAME="$(ps -o comm= -p "$APP_PID" | xargs basename)"
app_start="$(ps -o lstart= -p "$APP_PID")"
app_start_s="$(date -j -f '%a %b %d %T %Y' "$app_start" +%s)"
OUT="resize-storm-$(date +%Y%m%d-%H%M%S).csv"

# Physical footprint in MB (footprint(1); vmmap's summary as a fallback).
fp() {
    local v
    v="$(footprint -p "$1" 2>/dev/null | sed -nE 's/.*[Ff]ootprint: *([0-9.]+) *([KMG]B).*/\1 \2/p' | head -1)"
    if [ -z "$v" ]; then
        v="$(vmmap --summary "$1" 2>/dev/null | sed -nE 's/^Physical footprint: *([0-9.]+)([KMG]).*/\1 \2B/p' | head -1)"
    fi
    [ -n "$v" ] || { echo 0; return; }
    set -- $v
    case "$2" in KB) echo "$1 / 1024" | bc -l ;; GB) echo "$1 * 1024" | bc -l ;; *) echo "$1" ;; esac
}

# WebKit processes started after K2 (close Safari and other WebKit apps first).
webkit_pids() {
    ps -axo pid=,lstart=,comm= | while read -r pid d1 d2 d3 d4 d5 comm; do
        case "$comm" in *com.apple.WebKit.*) ;; *) continue ;; esac
        s="$(date -j -f '%a %b %d %T %Y' "$d1 $d2 $d3 $d4 $d5" +%s 2>/dev/null || echo 0)"
        [ "$s" -ge "$app_start_s" ] && echo "$pid:$(basename "$comm")"
    done
}

echo "t,phase,app_mb,webkit_total_mb,max_webcontent_mb,total_mb" > "$OUT"
sample() { # phase -> prints total, max_webcontent
    local app wk=0 mx=0 v
    app="$(fp "$APP_PID")"
    for e in $(webkit_pids); do
        v="$(fp "${e%%:*}")"
        wk="$(echo "$wk + $v" | bc -l)"
        case "${e#*:}" in *WebContent*) if [ "$(echo "$v > $mx" | bc -l)" = 1 ]; then mx="$v"; fi ;; esac
    done
    local total; total="$(echo "$app + $wk" | bc -l)"
    printf '%s,%s,%.1f,%.1f,%.1f,%.1f\n' "$(date +%s)" "$1" "$app" "$wk" "$mx" "$total" >> "$OUT"
    printf '%.1f %.1f\n' "$total" "$mx"
}

resize() { # w h
    osascript -e "tell application \"System Events\" to tell (first process whose unix id is $APP_PID) to set size of front window to {$1, $2}" >/dev/null
}

read -r base _ < <(sample pre)
echo "resize-storm: K2 pid $APP_PID ($PROC_NAME), pre-storm total ${base} MB; storming ${SECS}s"
peak=0; peak_wc=0
end=$(( $(date +%s) + SECS )); next=$(date +%s); i=0
while [ "$(date +%s)" -lt "$end" ]; do
    # 20 size changes a second, a slow sweep between 900x600 and 1700x1050.
    phase=$(( i % 80 )); [ "$phase" -ge 40 ] && phase=$(( 80 - phase ))
    resize $(( 900 + phase * 20 )) $(( 600 + phase * 11 ))
    i=$(( i + 1 ))
    if [ "$(date +%s)" -ge "$next" ]; then
        read -r t m < <(sample storm)
        [ "$(echo "$t > $peak" | bc -l)" = 1 ] && peak="$t"
        [ "$(echo "$m > $peak_wc" | bc -l)" = 1 ] && peak_wc="$m"
        next=$(( next + 1 ))
    fi
    sleep 0.05
done
resize 1400 900
back_at=""
for s in $(seq 1 "$SETTLE"); do
    sleep 1
    read -r t m < <(sample settle)
    [ "$(echo "$m > $peak_wc" | bc -l)" = 1 ] && peak_wc="$m"
    if [ -z "$back_at" ] && [ "$(echo "$t <= $base * 1.2" | bc -l)" = 1 ]; then back_at="$s"; fi
done
fail=0
rule() { if [ "$2" = 1 ]; then echo "PASS  $1"; else echo "FAIL  $1"; fail=1; fi; }
rule "total peak ${peak} MB <= 500 MB" "$(echo "$peak <= 500" | bc -l)"
rule "largest WebContent ${peak_wc} MB <= 300 MB" "$(echo "$peak_wc <= 300" | bc -l)"
rule "back within 20% of ${base} MB within ${SETTLE}s (at ${back_at:-never}s)" "$([ -n "$back_at" ] && echo 1 || echo 0)"
echo "resize-storm: samples in $OUT"
exit "$fail"
